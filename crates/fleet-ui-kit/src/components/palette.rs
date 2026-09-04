//! `Palette` — sectioned `GO` / `DO` / `CONTEXT` results with right-aligned key hints.
//!
//! §3.9. Three rules are structural rather than stylistic:
//!
//! * `GO` (objects) always comes **first**, so a session is reachable from inside another
//!   session with `ctrl-s s : pay fix ⏎` and no list scan;
//! * every `DO` row shows its bound key, right-aligned, so the palette trains itself out of
//!   the loop;
//! * the total cap is **10 rows** across all sections, so the top match never moves below the
//!   fold and `Enter` stays predictable. A command that is invalid here is **not listed at
//!   all** — never greyed, because a greyed row costs a `j`.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{
    components::{ColumnAlign, Row, RowColumn, TextField},
    icons::{Icon, IconSize},
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

    /// Prefix with `triangle-alert`. The action is still routed through its confirm dialog.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
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
}

/// The command palette card. Wrap it in an [`super::Overlay`].
#[derive(IntoElement)]
pub struct Palette {
    query: SharedString,
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

    /// How many rows will actually render, after the cap.
    pub fn shown(&self) -> usize {
        self.sections
            .iter()
            .map(|s| s.rows.len())
            .sum::<usize>()
            .min(self.cap)
    }
}

impl RenderOnce for Palette {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let shown = self.shown();
        let total = self.total.max(shown);
        let cursor = self.cursor;
        let cap = self.cap;

        let mut sections = self.sections;
        sections.sort_by_key(|s| s.kind);

        let mut flat = 0usize;
        let mut blocks: Vec<AnyElement> = Vec::new();
        for section in sections {
            if flat >= cap {
                break;
            }
            let mut rows: Vec<AnyElement> = Vec::new();
            for row in section.rows {
                if flat >= cap {
                    break;
                }
                let selected = flat == cursor;
                let mut list_row = Row::new()
                    .height(theme.metrics.palette_row_h)
                    .selected(selected)
                    .cursor(selected);
                if let Some(leading) = row.leading {
                    list_row = list_row.leading(leading);
                } else if let Some(icon) = row.icon {
                    list_row = list_row.leading(
                        icon.el()
                            .size(IconSize::Large)
                            .color(if row.destructive {
                                theme.colors.danger
                            } else {
                                theme.colors.text_secondary
                            }),
                    );
                }
                list_row = list_row.column(RowColumn::flex(Text::ui(row.label).ellipsize()));
                if let Some(detail) = row.detail {
                    list_row = list_row.column(RowColumn::auto(Text::ui(detail).faint()));
                }
                if let Some(key) = row.key {
                    list_row = list_row.column(
                        RowColumn::fixed(crate::theme::ch(4.0), Text::hint(key))
                            .align(ColumnAlign::Right),
                    );
                }
                rows.push(list_row.into_any_element());
                flat += 1;
            }
            if rows.is_empty() {
                continue;
            }
            blocks.push(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(
                        div()
                            .h(theme.metrics.section_header_h)
                            .flex()
                            .items_center()
                            .px(theme.space.md)
                            .child(Text::label(section.kind.title())),
                    )
                    .children(rows)
                    .into_any_element(),
            );
        }

        let body: AnyElement = if blocks.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(theme.metrics.palette_row_h)
                .child(Text::ui(self.empty.unwrap_or_default()).muted())
                .into_any_element()
        } else {
            div().flex().flex_col().children(blocks).into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .px(theme.space.md)
                    .py(theme.space.xs)
                    .border_b(gpui::px(1.0))
                    .border_color(theme.colors.border)
                    .child(
                        TextField::new(self.query)
                            .icon(Icon::ChevronRight)
                            .focused(true),
                    ),
            )
            .child(body)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .px(theme.space.md)
                    .py(theme.space.xs)
                    .border_t(gpui::px(1.0))
                    .border_color(theme.colors.border)
                    .child(Text::hint(format!("{shown} of {total}")).tone(Tone::Muted))
                    .child(crate::components::KeyHintRow::new()
                        .key("\u{23ce}", "run")
                        .key("esc", "cancel")),
            )
    }
}
