//! `PaneHeader` — label · scope · shown/total · visible range · stale stamp.
//!
//! §2.10. The filter bar replaces this header **in place**, in the same 30 px row, with zero
//! layout shift (§3.10); pass it through [`PaneHeader::filter`] instead of swapping elements.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The 30 px header of a pane.
#[derive(IntoElement)]
pub struct PaneHeader {
    label: SharedString,
    scope: Option<SharedString>,
    shown: Option<usize>,
    total: Option<usize>,
    range: Option<(usize, usize)>,
    stale_age: Option<SharedString>,
    filter_chip: Option<SharedString>,
    filter: Option<AnyElement>,
    trailing: Option<AnyElement>,
}

impl PaneHeader {
    /// A header with a label, e.g. `WORKTREES`.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            scope: None,
            shown: None,
            total: None,
            range: None,
            stale_age: None,
            filter_chip: None,
            filter: None,
            trailing: None,
        }
    }

    /// The scope suffix, e.g. `payroll` in `WORKTREES · payroll`.
    pub fn scope(mut self, scope: impl Into<SharedString>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// The total row count. Renders alone when no filter is active.
    pub fn total(mut self, total: usize) -> Self {
        self.total = Some(total);
        self
    }

    /// The filtered count. Renders as `shown/total`.
    pub fn shown(mut self, shown: usize) -> Self {
        self.shown = Some(shown);
        self
    }

    /// The visible scroll range, `first–last`, 1-based.
    pub fn range(mut self, first: usize, last: usize) -> Self {
        self.range = Some((first, last));
        self
    }

    /// Append `· stale · <age>` in amber (§1.3, §3.12).
    pub fn stale(mut self, age: impl Into<SharedString>) -> Self {
        self.stale_age = Some(age.into());
        self
    }

    /// The retained-filter chip shown after the label once the input is exited (`⌕rut`).
    pub fn filter_chip(mut self, query: impl Into<SharedString>) -> Self {
        self.filter_chip = Some(query.into());
        self
    }

    /// Replace the header's left side with a live [`super::FilterBar`], in place.
    pub fn filter(mut self, filter: impl IntoElement) -> Self {
        self.filter = Some(filter.into_any_element());
        self
    }

    /// An extra right-aligned element (the PR screen's fetch stamp).
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl RenderOnce for PaneHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let counts = match (self.shown, self.total) {
            (Some(shown), Some(total)) => Some(SharedString::from(format!("{shown}/{total}"))),
            (None, Some(total)) => Some(SharedString::from(total.to_string())),
            _ => None,
        };
        let range = self.range.zip(self.total).map(|((first, last), total)| {
            SharedString::from(format!("{first}\u{2013}{last}/{total}"))
        });

        let left: AnyElement = match self.filter {
            Some(filter) => filter,
            None => div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .min_w_0()
                .child(Text::label(self.label))
                .children(
                    self.scope
                        .map(|scope| Text::label(format!("\u{b7} {scope}")).faint()),
                )
                .children(self.filter_chip.map(|query| {
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.xxs)
                        .child(
                            Icon::Search
                                .el()
                                .size(IconSize::Small)
                                .color(theme.colors.accent),
                        )
                        .child(Text::hint(query).tone(Tone::Accent))
                }))
                .children(self.stale_age.map(|age| {
                    Text::label(format!("\u{b7} stale \u{b7} {age}")).tone(Tone::Warning)
                }))
                .into_any_element(),
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .size_full()
            .px(theme.space.lg)
            .gap(theme.space.md)
            .border_b(gpui::px(1.0))
            .border_color(theme.colors.border)
            .child(left)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.md)
                    .children(counts.map(|counts| Text::label(counts).faint()))
                    .children(range.map(|range| Text::label(range).faint()))
                    .children(self.trailing),
            )
    }
}
