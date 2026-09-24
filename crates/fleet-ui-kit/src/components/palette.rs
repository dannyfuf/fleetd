//! `Palette` — one ranked, scrolling list of commands and objects under a live query.
//!
//! §3.9. The palette is a query row over a single [`FuzzyList`] that carries section headings,
//! and a footer naming what `⏎` runs. What is structural rather than stylistic:
//!
//! * the owner ranks, the palette draws: sections arrive in the order the ranker put them, and
//!   the rows inside each one are already sorted, so the top row is always the best match and
//!   the flat cursor starts on it;
//! * the list scrolls past [`Palette::visible_rows`] instead of dropping rows, and a heading
//!   rides above its first row inside the same list item, so the cursor, the scroll position
//!   ([`Palette::reveal`]) and the harness numbering (`palette.row[N]`) all count rows only;
//! * every command row shows its own key as a [`Kbd`], so the palette trains itself out of the
//!   loop, and a destructive command wears a danger tile — it still goes through its confirm;
//! * a press on a row runs it, exactly as `⏎` does with the cursor on it.
//!
//! Ranking, matching and the flat cursor belong to the caller; [`Palette::flat_len`] gives it
//! the length to move that cursor with [`FuzzyList::next_cursor`] / [`FuzzyList::prev_cursor`].
//!
//! The query is a [`TextInput`] the caller owns, built embedded
//! ([`TextInput::set_embedded`]) because the query row is the palette's own chrome and a framed
//! field inside it would draw a second box.

use gpui::{
    Action, AnyElement, App, Entity, FontWeight, ScrollHandle, SharedString, Window, div,
    prelude::*,
};
use std::rc::Rc;

use crate::{
    components::{Chip, FuzzyItem, FuzzyList, Kbd, KbdSize, TextInput},
    harness::HarnessTargetExt as _,
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// One palette row.
pub struct PaletteRow {
    icon: Option<Icon>,
    leading: Option<AnyElement>,
    item: FuzzyItem,
    destructive: bool,
}

impl PaletteRow {
    /// A row.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            leading: None,
            item: FuzzyItem::new(label),
            destructive: false,
        }
    }

    /// The glyph inside the row's tile. Use the same glyph the object or action uses elsewhere.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A custom element inside the row's tile instead of a glyph, e.g. a
    /// [`super::StatusGlyph`] for a worktree row.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// The muted right-hand description: the place a command acts in (`Board`), a worktree's
    /// state (`session attached`), or `asks first` on a destructive command.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.item = self.item.trailing(detail);
        self
    }

    /// A muted qualifier on the label's own line, read with it (`· spike`).
    pub fn qualifier(mut self, qualifier: impl Into<SharedString>) -> Self {
        self.item = self.item.detail(qualifier);
        self
    }

    /// A muted second line, e.g. `blocked · question` or `working · 14m`.
    pub fn secondary(mut self, secondary: impl Into<SharedString>) -> Self {
        self.item = self.item.secondary(secondary);
        self
    }

    /// A right-aligned value, e.g. `attach` or `go`. Shares the slot with
    /// [`PaletteRow::detail`]; a row carries one or the other.
    pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self {
        self.item = self.item.trailing(trailing);
        self
    }

    /// A status word after the label, e.g. a card's column (`Todo`).
    pub fn badge(mut self, badge: impl Into<SharedString>) -> Self {
        self.item = self.item.badge(badge);
        self
    }

    /// The row's own key, right-aligned as chips. Resolved from the keymap by the owner.
    pub fn kbd(mut self, kbd: Kbd) -> Self {
        self.item = self.item.kbd(kbd);
        self
    }

    /// The character indices of the label the ranker matched, drawn as a weight bump.
    pub fn matches(mut self, matches: impl IntoIterator<Item = usize>) -> Self {
        self.item = self.item.matches(matches);
        self
    }

    /// Draw the tile in the danger wash: the command deletes or stops something. The action
    /// is still routed through its confirm; say so with [`PaletteRow::detail`] (`asks first`).
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// The [`FuzzyItem`] this row renders as, with its tile as the leading element.
    fn into_item(self, heading: Option<SharedString>, theme: &Theme) -> FuzzyItem {
        let (fill, color) = if self.destructive {
            (Tone::Danger.fill(theme), theme.colors.danger)
        } else {
            (theme.colors.control, theme.colors.text_secondary)
        };
        let content = self.leading.or_else(|| {
            self.icon.map(|icon| {
                icon.el()
                    .size(IconSize::Small)
                    .color(color)
                    .into_any_element()
            })
        });
        let tile = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(theme.metrics.palette_tile)
            .rounded(theme.radii.sm)
            .bg(fill)
            .children(content);
        let item = self.item.leading(tile);
        match heading {
            Some(heading) => item.heading(heading),
            None => item,
        }
    }
}

impl FluentBuilder for PaletteRow {}

/// One titled run of rows. Sections render in the order they are added.
pub struct PaletteSection {
    title: SharedString,
    rows: Vec<PaletteRow>,
}

impl PaletteSection {
    /// A section under a sentence-case `title` (`Commands`, `Go to`).
    pub fn new(title: impl Into<SharedString>, rows: impl IntoIterator<Item = PaletteRow>) -> Self {
        Self {
            title: title.into(),
            rows: rows.into_iter().collect(),
        }
    }

    /// How many rows this section holds.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the section would render nothing.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// `(flat index, window, cx)`: run the clicked row.
type RowHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The command palette card. Wrap it in an [`super::Overlay`].
#[derive(IntoElement)]
pub struct Palette {
    input: Entity<TextInput>,
    sections: Vec<PaletteSection>,
    cursor: usize,
    visible_rows: usize,
    scroll: Option<ScrollHandle>,
    scope: Option<SharedString>,
    prefix_hint: Vec<(SharedString, SharedString)>,
    selected_label: Option<SharedString>,
    run_action: Option<Box<dyn Action>>,
    empty: Option<SharedString>,
    on_click: Option<RowHandler>,
}

impl Palette {
    /// A palette over ranked sections, editing through `input`.
    pub fn new(input: Entity<TextInput>) -> Self {
        Self {
            input,
            sections: Vec::new(),
            cursor: 0,
            visible_rows: 10,
            scroll: None,
            scope: None,
            prefix_hint: Vec::new(),
            selected_label: None,
            run_action: None,
            empty: None,
            on_click: None,
        }
    }

    /// Append a section, below the ones already added.
    pub fn section(mut self, section: PaletteSection) -> Self {
        self.sections.push(section);
        self
    }

    /// The flat cursor index across all sections.
    pub fn cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor;
        self
    }

    /// How many rows tall the results are before they scroll. 10 by default.
    pub fn visible_rows(mut self, rows: usize) -> Self {
        self.visible_rows = rows;
        self
    }

    /// Track the results' scroll position, so the owner can [`Palette::reveal`] the cursor.
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// The scope chip after the query (`All`, `Commands`): what the typed prefix narrowed to.
    pub fn scope(mut self, scope: impl Into<SharedString>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// The prefix legend (`type > commands · @ worktrees · # cards`), as `(prefix, meaning)`
    /// pairs. The owner passes it while the query is empty and nothing once typing starts.
    pub fn prefix_hint(
        mut self,
        hint: impl IntoIterator<Item = (impl Into<SharedString>, impl Into<SharedString>)>,
    ) -> Self {
        self.prefix_hint = hint
            .into_iter()
            .map(|(prefix, meaning)| (prefix.into(), meaning.into()))
            .collect();
        self
    }

    /// The footer's left half: the label of the row `⏎` would run.
    pub fn selected_label(mut self, label: impl Into<SharedString>) -> Self {
        self.selected_label = Some(label.into());
        self
    }

    /// The action `⏎` dispatches. The footer's `Run` chip is resolved from its live binding.
    pub fn run_action(mut self, action: Box<dyn Action>) -> Self {
        self.run_action = Some(action);
        self
    }

    /// The no-match line, e.g. `Nothing matches "gpu".`
    pub fn empty(mut self, empty: impl Into<SharedString>) -> Self {
        self.empty = Some(empty.into());
        self
    }

    /// A press on a row runs it. The handler receives the **flat** index the cursor uses, so
    /// it can do exactly what `⏎` does with the cursor on that row.
    pub fn on_click(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// How many rows exist across every section: the length `ctrl-n` / `ctrl-p` wrap around.
    pub fn flat_len(&self) -> usize {
        self.sections.iter().map(PaletteSection::len).sum()
    }

    /// Scroll `handle` so the row at flat index `cursor` (with its heading, when it opens a
    /// section) is in view. Call it from the action that moved the cursor, never from `render`.
    pub fn reveal(handle: &ScrollHandle, cursor: usize) {
        FuzzyList::reveal(handle, cursor);
    }
}

impl RenderOnce for Palette {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let run_kbd = self
            .run_action
            .as_deref()
            .and_then(|action| Kbd::for_action(action, window, cx))
            .map(|kbd| kbd.size(KbdSize::Small));

        let items: Vec<FuzzyItem> = self
            .sections
            .into_iter()
            .flat_map(|section| {
                let title = section.title;
                section
                    .rows
                    .into_iter()
                    .enumerate()
                    .map(move |(index, row)| ((index == 0).then(|| title.clone()), row))
            })
            .map(|(heading, row)| row.into_item(heading, &theme))
            .collect();

        let query_style = TextRole::Title.style(&theme);
        let query = styled_with(div(), query_style, &theme)
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .h(query_style.line_height)
            .overflow_hidden()
            .font_weight(FontWeight::NORMAL)
            .text_color(theme.colors.text)
            .child(self.input)
            .harness_target("palette.input");

        let hint = (!self.prefix_hint.is_empty()).then(|| {
            let last = self.prefix_hint.len() - 1;
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space.xxs)
                .child(Text::caption("type").muted())
                .children(self.prefix_hint.into_iter().enumerate().flat_map(
                    move |(index, (prefix, meaning))| {
                        [
                            Text::hint(prefix)
                                .color(theme.colors.text_secondary)
                                .into_any_element(),
                            Text::caption(meaning).muted().into_any_element(),
                        ]
                        .into_iter()
                        .chain(
                            (index < last)
                                .then(|| Text::caption("\u{b7}").faint().into_any_element()),
                        )
                    },
                ))
        });

        let mut results = FuzzyList::new("palette-results", items)
            .cursor(self.cursor)
            .under_text_field(true)
            .visible_rows(self.visible_rows)
            .row_height(theme.metrics.palette_row_h)
            .leading_width(theme.metrics.palette_tile)
            // One flat numbering across every section, so `palette.row[0]` is the top match
            // whichever section it came from — the same counting the cursor uses.
            .harness_rows("palette.row", 0)
            .empty(Text::ui(self.empty.unwrap_or_default()).muted());
        if let Some(handle) = self.scroll.as_ref() {
            results = results.track_scroll(handle);
        }
        if let Some(run) = self.on_click {
            results = results.on_click(move |ix, window, cx| run(ix, window, cx));
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.sm)
                    .w_full()
                    .h(theme.metrics.palette_input_h)
                    .px(theme.space.lg)
                    .border_b(theme.metrics.hairline)
                    .border_color(theme.colors.border)
                    .child(
                        Icon::Search
                            .el()
                            .size(IconSize::Large)
                            .color(theme.colors.text_secondary),
                    )
                    .child(query)
                    .children(self.scope.map(|scope| Chip::new().text(scope).filled(true)))
                    .children(hint),
            )
            .child(div().w_full().py(theme.space.xs).child(results))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.md)
                    .w_full()
                    .h(theme.metrics.row_h)
                    .px(theme.space.md)
                    .border_t(theme.metrics.hairline)
                    .border_color(theme.colors.border)
                    .bg(theme.colors.chrome)
                    .child(
                        div().flex().flex_1().min_w_0().children(
                            self.selected_label
                                .map(|label| Text::caption(label).muted().ellipsize()),
                        ),
                    )
                    .children(run_kbd.map(|kbd| {
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(theme.space.xs)
                            .child(Text::caption("Run").faint())
                            .child(kbd)
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::InputMode;

    fn section(title: &'static str, n: usize) -> PaletteSection {
        PaletteSection::new(title, (0..n).map(|i| PaletteRow::new(format!("row {i}"))))
    }

    #[gpui::test]
    fn every_row_of_every_section_is_counted(cx: &mut gpui::TestAppContext) {
        let input = cx.new(|cx| TextInput::new(InputMode::SingleLine, cx));
        let palette = Palette::new(input)
            .section(section("Commands", 6))
            .section(section("Go to", 8))
            .section(section("Cards", 3));
        assert_eq!(palette.flat_len(), 17);
    }

    #[test]
    fn an_empty_section_is_empty() {
        assert!(section("Cards", 0).is_empty());
        assert_eq!(section("Cards", 2).len(), 2);
    }
}
