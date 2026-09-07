//! `SegmentedTabs` — underlined tabs with counts. Fleet's only tab bar outside the Workspace.
//!
//! §3.5: `MINE 7` / `REVIEW 4`, active tab marked with a 2 px accent underline, moved with
//! `Tab` / `S-Tab` / `h` / `l`. The counts answer "how much is queued" without entering.
//!
//! A tab is **not** a chip. `Some(0)` renders `0`, because §1.2's zero-suppression is about
//! chrome that would otherwise say nothing; an empty tab must still say it is empty, or the
//! user reads the missing number as "not loaded yet". `loading(true)` is the state that means
//! *that*, and it keeps the cached rows at full opacity while it shows `…`.

use std::rc::Rc;

use gpui::{App, MouseButton, SharedString, Window, div, prelude::*};

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

    /// What the count column renders, if anything.
    pub fn count_text(&self) -> Option<SharedString> {
        if self.loading {
            return Some(SharedString::new_static("\u{2026}"));
        }
        self.count
            .map(|count| SharedString::from(count.to_string()))
    }
}

/// A row of underlined tabs.
#[derive(IntoElement)]
pub struct SegmentedTabs {
    tabs: Vec<SegmentedTab>,
    active: usize,
    underlined: bool,
    on_select: Option<Rc<TabSelect>>,
}

type TabSelect = dyn Fn(usize, &mut Window, &mut App);

impl SegmentedTabs {
    /// A tab bar.
    pub fn new(tabs: impl IntoIterator<Item = SegmentedTab>) -> Self {
        Self {
            tabs: tabs.into_iter().collect(),
            active: 0,
            underlined: true,
            on_select: None,
        }
    }

    /// Use a selected background instead of an underline for a parent navigation level.
    pub fn underlined(mut self, underlined: bool) -> Self {
        self.underlined = underlined;
        self
    }

    /// Handles mouse selection with the same intent as keyboard tab navigation.
    pub fn on_select(mut self, on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Which tab is active.
    pub fn active(mut self, active: usize) -> Self {
        self.active = active;
        self
    }

    /// How many tabs there are.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether the bar has no tabs at all.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// `Tab` / `l`: the next tab, wrapping — two tabs must be reachable with one repeated key.
    pub fn next_index(active: usize, len: usize) -> usize {
        super::navigation::next(active, len)
    }

    /// `S-Tab` / `h`: the previous tab, wrapping.
    pub fn prev_index(active: usize, len: usize) -> usize {
        super::navigation::previous(active, len)
    }
}

impl RenderOnce for SegmentedTabs {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active;
        let underline_h = theme.metrics.focus_ring_w;
        let accent = theme.colors.accent;

        div()
            .flex()
            .items_stretch()
            .flex_none()
            .h(theme.metrics.pane_header_h)
            .gap(theme.space.xl)
            .children(self.tabs.into_iter().enumerate().map(move |(ix, tab)| {
                let is_active = ix == active;
                let count = tab.count_text();
                let count_tone = if tab.loading {
                    Tone::Warning
                } else if is_active {
                    Tone::Secondary
                } else {
                    Tone::Muted
                };
                let on_select = self.on_select.clone();
                div()
                    .id(tab.label.clone())
                    .when_some(on_select, |el, on_select| {
                        el.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            move |_, window, cx| {
                                on_select(ix, window, cx);
                                cx.stop_propagation();
                            },
                        )
                    })
                    .when(!self.underlined, |el| {
                        el.px(theme.space.sm)
                            .rounded(theme.radii.sm)
                            .when(is_active, |el| el.bg(theme.colors.row_selected))
                    })
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .gap(theme.space.sm)
                            .child(
                                Text::label(tab.label)
                                    .tone(if is_active {
                                        Tone::Default
                                    } else {
                                        Tone::Secondary
                                    })
                                    .weight(if is_active {
                                        theme.text.ui_strong.weight
                                    } else {
                                        theme.text.ui.weight
                                    }),
                            )
                            .children(count.map(|count| Text::label(count).tone(count_tone))),
                    )
                    // The underline slot exists on every tab so the active one does not
                    // shift the row by 2 px when it moves.
                    .child(
                        div()
                            .h(underline_h)
                            .w_full()
                            .bg(if is_active && self.underlined {
                                accent
                            } else {
                                gpui::transparent_black()
                            }),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_tab_still_says_zero() {
        assert_eq!(
            SegmentedTab::new("mine", 0).count_text().as_deref(),
            Some("0")
        );
        assert_eq!(SegmentedTab::bare("help").count_text(), None);
    }

    #[test]
    fn loading_replaces_the_count_with_an_ellipsis() {
        let tab = SegmentedTab::new("review", 4).loading(true);
        assert_eq!(tab.count_text().as_deref(), Some("\u{2026}"));
    }

    #[test]
    fn tab_motion_wraps_both_ways() {
        assert_eq!(SegmentedTabs::next_index(1, 2), 0);
        assert_eq!(SegmentedTabs::prev_index(0, 2), 1);
        assert_eq!(SegmentedTabs::next_index(0, 0), 0);
        assert_eq!(SegmentedTabs::prev_index(0, 0), 0);
    }
}
