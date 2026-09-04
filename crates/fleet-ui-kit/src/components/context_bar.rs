//! `ContextBar` — numbered tabs, overflow chip, chip tray, daemon dot.
//!
//! §2.2 and §3.1. The bar is the 36 px unified titlebar, so the tabs start at x = 84 to clear
//! the macOS traffic lights. The active tab carries a 2 px accent underline, which together
//! with the cursor bar is the only blue in the app.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*, px};

use crate::{
    components::{DaemonDot, DaemonState},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// One context tab: a name and the digit that jumps to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextTab {
    /// The context name.
    pub label: SharedString,
    /// The `1`-`9` digit, shown faint after the name. `None` past nine contexts.
    pub index_hint: Option<SharedString>,
}

impl ContextTab {
    /// A tab with a digit.
    pub fn new(label: impl Into<SharedString>, index: usize) -> Self {
        Self {
            label: label.into(),
            index_hint: (index <= 9).then(|| SharedString::from(index.to_string())),
        }
    }

    /// A tab with no digit.
    pub fn unnumbered(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            index_hint: None,
        }
    }
}

/// The top bar.
#[derive(IntoElement)]
pub struct ContextBar {
    tabs: Vec<ContextTab>,
    active: usize,
    overflow: usize,
    chips: Vec<AnyElement>,
    daemon: DaemonState,
    daemon_label: Option<SharedString>,
    leading_inset: gpui::Pixels,
    empty_message: Option<(SharedString, SharedString)>,
}

impl ContextBar {
    /// A bar over the given tabs.
    pub fn new(tabs: impl IntoIterator<Item = ContextTab>) -> Self {
        Self {
            tabs: tabs.into_iter().collect(),
            active: 0,
            overflow: 0,
            chips: Vec::new(),
            daemon: DaemonState::Healthy,
            daemon_label: None,
            leading_inset: px(84.0),
            empty_message: None,
        }
    }

    /// Index of the active tab.
    pub fn active(mut self, active: usize) -> Self {
        self.active = active;
        self
    }

    /// How many contexts are past tab 9. Rendered as a faint `+n`.
    pub fn overflow(mut self, overflow: usize) -> Self {
        self.overflow = overflow;
        self
    }

    /// Append a status chip. Zero-suppressed chips render nothing, so pass them all.
    pub fn chip(mut self, chip: impl IntoElement) -> Self {
        self.chips.push(chip.into_any_element());
        self
    }

    /// The daemon dot state.
    pub fn daemon(mut self, state: DaemonState) -> Self {
        self.daemon = state;
        self
    }

    /// The word next to a degraded daemon dot.
    pub fn daemon_label(mut self, label: impl Into<SharedString>) -> Self {
        self.daemon_label = Some(label.into());
        self
    }

    /// Left inset. 84 px on macOS to clear the traffic lights; 12 px elsewhere.
    pub fn leading_inset(mut self, inset: gpui::Pixels) -> Self {
        self.leading_inset = inset;
        self
    }

    /// The "no contexts" rendering: a fact line and a key line, replacing the tabs.
    pub fn empty(
        mut self,
        fact: impl Into<SharedString>,
        action: impl Into<SharedString>,
    ) -> Self {
        self.empty_message = Some((fact.into(), action.into()));
        self
    }
}

impl RenderOnce for ContextBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active;
        let mut dot = DaemonDot::new(self.daemon);
        if let Some(label) = self.daemon_label {
            dot = dot.label(label);
        }

        let left: AnyElement = match self.empty_message {
            Some((fact, action)) => div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(Text::ui(fact).faint())
                .child(Text::hint(action).faint())
                .into_any_element(),
            None => div()
                .flex()
                .items_center()
                .gap(theme.space.lg)
                .children(self.tabs.into_iter().enumerate().map(|(ix, tab)| {
                    let is_active = ix == active;
                    div()
                        .flex()
                        .flex_col()
                        .justify_between()
                        .h_full()
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .items_center()
                                .gap(theme.space.xs)
                                .px(theme.space.sm)
                                .child(if is_active {
                                    Text::ui_strong(tab.label)
                                } else {
                                    Text::ui(tab.label).muted()
                                })
                                .children(
                                    tab.index_hint.map(|hint| Text::hint(hint).faint()),
                                ),
                        )
                        .child(
                            div()
                                .h(theme.metrics.focus_ring_w)
                                .w_full()
                                .bg(if is_active {
                                    theme.colors.accent
                                } else {
                                    gpui::transparent_black()
                                }),
                        )
                }))
                .children((self.overflow > 0).then(|| {
                    Text::hint(format!("+{}", self.overflow)).tone(Tone::Muted)
                }))
                .into_any_element(),
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .size_full()
            .pl(self.leading_inset)
            .pr(theme.space.md)
            .gap(theme.space.md)
            .bg(theme.colors.bg)
            .border_b(px(1.0))
            .border_color(theme.colors.border)
            .child(left)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.sm)
                    .children(self.chips)
                    .child(div().w(theme.space.xs))
                    .child(dot),
            )
    }
}
