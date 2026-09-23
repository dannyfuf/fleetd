//! `PageHeader` — the top of a Hub page: an H1, one summary line under it, and the page's
//! toolbar on the right.
//!
//! ```text
//! Worktrees                                  [⩸ Filter  /] [Clone repo] [+ New worktree n]
//! 4 across 2 repositories · 1 needs attention
//! ```
//!
//! The title is the page's name in `page_title`; the subtitle is a sentence the view model
//! already built (counts, scope), never assembled here. The toolbar holds the page's own
//! controls — a [`super::FilterField`], secondary [`super::Button`]s and at most one primary
//! one — right-aligned to the title's baseline. A frozen snapshot adds an amber
//! `stale · <age>` after the subtitle (§1.3), the same stamp a [`super::PaneHeader`] carries.
//!
//! Not a [`super::PaneHeader`]: that is the 30 px label row of a dense pane. Not a
//! [`super::SectionHeader`], which names a block inside a panel.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// The top of a Hub page.
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    subtitle: Option<SharedString>,
    stale: Option<SharedString>,
    badge: Option<AnyElement>,
    facts: Vec<AnyElement>,
    actions: Vec<AnyElement>,
}

impl PageHeader {
    /// A header titled `title`, e.g. `Worktrees`.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            stale: None,
            badge: None,
            facts: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// The summary line under the title, already worded by the caller.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Mark the page as drawn from a frozen snapshot of this age (`12s`).
    pub fn stale(mut self, age: impl Into<SharedString>) -> Self {
        self.stale = Some(age.into());
        self
    }

    /// A small element after the title on its line: a board's prefix `Badge`.
    pub fn badge(mut self, badge: impl IntoElement) -> Self {
        self.badge = Some(badge.into_any_element());
        self
    }

    /// Append an element to the summary line, after the subtitle: a clickable `1 needs you`,
    /// a counter chip, a spinner. The words are still the caller's; this is where they sit.
    pub fn fact(mut self, fact: impl IntoElement) -> Self {
        self.facts.push(fact.into_any_element());
        self
    }

    /// Append a toolbar control, left to right; the toolbar is right-aligned.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let summary = (self.subtitle.is_some() || self.stale.is_some() || !self.facts.is_empty())
            .then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.xs)
                    .min_w_0()
                    .children(
                        self.subtitle
                            .map(|subtitle| Text::caption(subtitle).muted().ellipsize()),
                    )
                    .children(self.stale.map(|age| {
                        Text::caption(format!("\u{b7} stale \u{b7} {age}"))
                            .tone(Tone::Warning)
                            .flex_none()
                    }))
                    .children(self.facts)
            });
        div()
            .flex()
            .items_end()
            .w_full()
            .gap(theme.space.md)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(theme.space.xxs)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space.sm)
                            .min_w_0()
                            .child(Text::page_title(self.title).ellipsize())
                            .children(self.badge.map(|badge| div().flex_none().child(badge))),
                    )
                    .children(summary),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.sm)
                    .children(self.actions),
            )
    }
}
