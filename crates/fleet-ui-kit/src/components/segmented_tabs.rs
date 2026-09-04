//! `SegmentedTabs` — underlined tabs with counts. Fleet's only tab bar outside the Workspace.
//!
//! §3.5: `MINE 7` / `REVIEW 4`, active tab marked with a 2 px accent underline, moved with
//! `Tab` / `S-Tab` / `h` / `l`. The counts answer "how much is queued" without entering.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// One tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentedTab {
    /// The label. Rendered in the label type role, so it is uppercased.
    pub label: SharedString,
    /// The count. `None` renders no number; `Some(0)` renders `0` (a tab is not a chip: an
    /// empty tab must still say it is empty).
    pub count: Option<usize>,
    /// Replace the count with `…` while a refresh is in flight.
    pub loading: bool,
}

impl SegmentedTab {
    /// A tab with a count.
    pub fn new(label: impl Into<SharedString>, count: usize) -> Self {
        Self {
            label: label.into(),
            count: Some(count),
            loading: false,
        }
    }

    /// A tab with no count.
    pub fn bare(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            count: None,
            loading: false,
        }
    }

    /// Show `…` instead of the count.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }
}

/// A row of underlined tabs.
#[derive(IntoElement)]
pub struct SegmentedTabs {
    tabs: Vec<SegmentedTab>,
    active: usize,
}

impl SegmentedTabs {
    /// A tab bar.
    pub fn new(tabs: impl IntoIterator<Item = SegmentedTab>) -> Self {
        Self {
            tabs: tabs.into_iter().collect(),
            active: 0,
        }
    }

    /// Which tab is active.
    pub fn active(mut self, active: usize) -> Self {
        self.active = active;
        self
    }
}

impl RenderOnce for SegmentedTabs {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active;
        div()
            .flex()
            .items_stretch()
            .h(theme.metrics.pane_header_h)
            .gap(theme.space.xl)
            .children(self.tabs.into_iter().enumerate().map(|(ix, tab)| {
                let is_active = ix == active;
                let count: Option<SharedString> = if tab.loading {
                    Some(SharedString::new_static("\u{2026}"))
                } else {
                    tab.count.map(|c| SharedString::from(c.to_string()))
                };
                div()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .gap(theme.space.sm)
                            .child(Text::label(tab.label).tone(if is_active {
                                Tone::Default
                            } else {
                                Tone::Secondary
                            }))
                            .children(count.map(|c| Text::label(c).faint())),
                    )
                    .child(div().h(theme.metrics.focus_ring_w).w_full().bg(
                        if is_active {
                            theme.colors.accent
                        } else {
                            gpui::transparent_black()
                        },
                    ))
            }))
    }
}
