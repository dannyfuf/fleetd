//! `SettingsCard` — a titled group of [`super::SettingsRow`]s on a raised surface.
//!
//! **Purpose.** A settings pane is a column of these: each card gathers the rows that answer one
//! question (`Claude`, `Keep awake while running`, `Prepare`), so the pane reads as a few named
//! groups instead of one long list. Rows with no heading go in an untitled card.
//!
//! **Anatomy.** `surface_raised` fill, a hairline `border`, `radii.card` corners, no shadow
//! (elevation 1). An optional 36 px header (`card_header_h`): the title in `ui_strong`, an optional
//! muted [`note`](SettingsCard::note) at its end, a hairline under it. A
//! [`subtitle`](SettingsCard::subtitle) stacks a muted sentence under the title and the header
//! grows to fit. Then the rows, separated by hairlines — never by gaps — and an optional last
//! [`caption`](SettingsCard::caption) line.
//!
//! **API.** [`SettingsCard::new`] takes the card's id; [`title`](SettingsCard::title),
//! [`note`](SettingsCard::note), [`subtitle`](SettingsCard::subtitle), [`row`](SettingsCard::row),
//! [`rows`](SettingsCard::rows) and [`caption`](SettingsCard::caption) are builders.
//!
//! **States.** The card has none of its own: cursor, hover, invalid and disabled belong to its
//! rows.
//!
//! **Usage rule.** Use it for a settings pane's groups only. A read-only block of facts is a
//! [`super::InfoCard`]; a floating surface is a [`super::Dialog`] or a [`super::Sheet`].

use gpui::{AnyElement, App, ElementId, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme};

/// A group of settings rows under an optional title.
#[derive(IntoElement)]
pub struct SettingsCard {
    id: ElementId,
    title: Option<SharedString>,
    note: Option<SharedString>,
    subtitle: Option<SharedString>,
    rows: Vec<AnyElement>,
    caption: Option<AnyElement>,
}

impl SettingsCard {
    /// An empty, untitled card.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            title: None,
            note: None,
            subtitle: None,
            rows: Vec::new(),
            caption: None,
        }
    }

    /// The card's heading: a 36 px header in `ui_strong`. Without one the card has no header.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A short muted caption at the header's end: `matched against running processes now`.
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// A muted sentence under the title, saying when or where the card's rows apply. The header
    /// grows to hold both lines. Use [`SettingsCard::note`] for a short trailing aside instead.
    pub fn subtitle(mut self, text: impl Into<SharedString>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    /// Append one row, normally a [`super::SettingsRow`].
    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }

    /// Append several rows.
    pub fn rows(mut self, rows: impl IntoIterator<Item = AnyElement>) -> Self {
        self.rows.extend(rows);
        self
    }

    /// A last line under the rows, at least 36 px tall: a broken rule in `danger`, or a note.
    pub fn caption(mut self, caption: impl IntoElement) -> Self {
        self.caption = Some(caption.into_any_element());
        self
    }
}

impl RenderOnce for SettingsCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let hairline = theme.metrics.hairline;
        let border = theme.colors.border;

        let header = self.title.map(|title| {
            let title_line = div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .min_w_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Text::ui_strong(title).ellipsize()),
                )
                .children(
                    self.note
                        .map(|note| Text::caption(note).muted().flex_none()),
                );
            div()
                .flex()
                .flex_col()
                .justify_center()
                .gap(theme.space.xxs)
                .px(theme.space.md)
                .map(|el| match self.subtitle {
                    // Two lines: the header grows past its 36 px floor to hold the sentence.
                    Some(subtitle) => el
                        .min_h(theme.metrics.card_header_h)
                        .py(theme.space.xs + theme.space.xxs)
                        .child(title_line)
                        .child(Text::caption(subtitle).muted()),
                    None => el.h(theme.metrics.card_header_h).child(title_line),
                })
                .into_any_element()
        });
        let caption = self.caption.map(|caption| {
            div()
                .flex()
                .items_center()
                .min_h(theme.metrics.card_header_h)
                .px(theme.space.md)
                .py(theme.space.xs)
                .child(caption)
                .into_any_element()
        });

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .bg(theme.colors.surface_raised)
            .border(hairline)
            .border_color(border)
            .rounded(theme.radii.card)
            .overflow_hidden()
            .children(
                header
                    .into_iter()
                    .chain(self.rows)
                    .chain(caption)
                    .enumerate()
                    // Hairlines between parts, never gaps: every part after the first carries
                    // the line above it.
                    .map(|(ix, part)| {
                        div()
                            .w_full()
                            .when(ix > 0, |el| el.border_t(hairline).border_color(border))
                            .child(part)
                    }),
            )
    }
}
