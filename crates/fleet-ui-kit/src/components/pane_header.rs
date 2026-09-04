//! `PaneHeader` — label · scope · `shown/total` · visible range · stale stamp.
//!
//! §2.10:
//!
//! ```text
//!  WORKTREES · payroll                              8/12          1–8/12
//!  └ label ──┘ └ scope ┘                       └ shown/total ┘ └ visible range ┘
//! ```
//!
//! The visible range is the scroll-position indicator the inventory requires and no proposal
//! supplied; it is what tells the user that `G` has somewhere to go.
//!
//! ## The filter must pass *through* the header
//!
//! §3.10 replaces the left side with the filter bar **in place**, in the same 30 px row, with
//! zero layout shift. Pass a [`super::FilterBar`] to [`PaneHeader::filter`] rather than
//! swapping the header element for one, or the row shifts by a pixel and the illusion that
//! "the list did not move" — the whole point of filtering in the header — breaks.
//!
//! ## States
//!
//! normal · filtering (`filter`) · filter retained (`filter_chip`, accent) · stale
//! (`· stale · 2m`, amber, §1.3 / §3.12). There is no focused, disabled or error state: the
//! header describes a pane, and the pane owns the focus ring.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*, px};

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
    /// A header with a label, e.g. `WORKTREES`. `Text::label` uppercases it — pass it as
    /// written.
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

    /// The filtered count. Renders as `shown/total`, and turns amber at `0` — a filter that
    /// matches nothing is the classic "where did my rows go" moment (§3.10).
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
        let no_match = self.shown == Some(0) && self.total.is_some_and(|total| total > 0);
        let counts = match (self.shown, self.total) {
            (Some(shown), Some(total)) => Some(SharedString::from(format!("{shown}/{total}"))),
            (None, Some(total)) => Some(SharedString::from(total.to_string())),
            _ => None,
        };
        let range = self.range.zip(self.total).map(|((first, last), total)| {
            SharedString::from(format!("{first}\u{2013}{last}/{total}"))
        });

        let left: AnyElement = match self.filter {
            Some(filter) => div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .child(filter)
                .into_any_element(),
            None => div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .min_w_0()
                .overflow_hidden()
                .child(Text::label(self.label))
                .children(
                    self.scope
                        .map(|scope| Text::label(format!("\u{b7} {scope}")).faint().ellipsize()),
                )
                .children(self.filter_chip.map(|query| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(theme.space.xxs)
                        .child(
                            Icon::Search
                                .el()
                                .size(IconSize::Medium)
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
            .border_b(px(1.0))
            .border_color(theme.colors.border)
            .child(left)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.md)
                    .children(counts.map(|counts| {
                        Text::label(counts).tone(if no_match { Tone::Warning } else { Tone::Muted })
                    }))
                    .children(range.map(|range| Text::label(range).faint()))
                    .children(self.trailing),
            )
    }
}
