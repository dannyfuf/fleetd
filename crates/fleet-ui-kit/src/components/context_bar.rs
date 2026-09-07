//! `ContextBar` — numbered tabs, overflow chip, chip tray, daemon dot.
//!
//! §2.2 and §3.1: *which slice of the world am I in, and is anything moving in it?* The bar is
//! the 36 px unified titlebar, so its tabs start at x = 84 to clear the macOS traffic lights
//! (12–72). The active tab carries a 2 px accent underline, which together with the cursor bar
//! is the only blue in the app — blue always answers "where am I", never "how is it going".
//!
//! ## Anatomy
//!
//! ```text
//! [84 px inset][ tab 1 ][ tab 2 ]…[ +n ]     …     [ status chips ][ 12 px ][ daemon dot ]
//! ```
//!
//! Tabs are `text + 8 px` padding with a 16 px gap; the overflow chip is the faint `+n` for
//! contexts past nine, which are reachable by `gt` / `gT` and the palette only.
//!
//! ## States
//!
//! default · empty (`No contexts yet.` + the key that fixes it) · daemon degraded (the dot
//! grows a labelled pill, §3.12).
//!
//! ## Keyboard
//!
//! `1`–`9` jump, `gt` / `gT` cycle, `N` new, `E` edit. The **screen** binds them, not the bar:
//! the digits are visible here to explain the screen's bindings.

use gpui::{AnyElement, App, Pixels, SharedString, Window, div, prelude::*, transparent_black};

use crate::{
    components::{DaemonDot, DaemonState},
    text::Text,
    theme::ActiveTheme,
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
    /// A tab with a digit. Indices above nine drop the hint: `1`–`9` is the whole binding.
    pub fn new(label: impl Into<SharedString>, index: usize) -> Self {
        Self {
            label: label.into(),
            index_hint: (1..=9)
                .contains(&index)
                .then(|| SharedString::from(index.to_string())),
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
    leading_inset: Option<Pixels>,
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
            leading_inset: None,
            empty_message: None,
        }
    }

    /// Index of the active tab.
    pub fn active(mut self, active: usize) -> Self {
        self.active = active;
        self
    }

    /// How many contexts are past tab 9. Rendered as a faint `+n`, zero-suppressed.
    pub fn overflow(mut self, overflow: usize) -> Self {
        self.overflow = overflow;
        self
    }

    /// Append a status chip. Zero-suppressed chips render nothing, so pass them all —
    /// including the ones that are currently `0`, which is what keeps the fixed §2.3 order
    /// stable as counts come and go.
    pub fn chip(mut self, chip: impl IntoElement) -> Self {
        self.chips.push(chip.into_any_element());
        self
    }

    /// The daemon dot state.
    pub fn daemon(mut self, state: DaemonState) -> Self {
        self.daemon = state;
        self
    }

    /// The word next to a degraded daemon dot, e.g. `fleetd stopped`.
    pub fn daemon_label(mut self, label: impl Into<SharedString>) -> Self {
        self.daemon_label = Some(label.into());
        self
    }

    /// Left inset. 84 px on macOS to clear the traffic lights; 12 px elsewhere.
    pub fn leading_inset(mut self, inset: Pixels) -> Self {
        self.leading_inset = Some(inset);
        self
    }

    /// The "no contexts" rendering: a fact line and a key line, replacing the tabs (§3.13).
    pub fn empty(mut self, fact: impl Into<SharedString>, action: impl Into<SharedString>) -> Self {
        self.empty_message = Some((fact.into(), action.into()));
        self
    }
}

impl RenderOnce for ContextBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active;
        let underline_h = theme.metrics.focus_ring_w;
        let mut dot = DaemonDot::new(self.daemon);
        if let Some(label) = self.daemon_label {
            dot = dot.label(label);
        }

        let left: AnyElement = match self.empty_message {
            Some((fact, action)) => div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .min_w_0()
                .child(Text::ui(fact).muted().ellipsize())
                .child(Text::hint(action).faint())
                .into_any_element(),
            None => div()
                .flex()
                .items_center()
                .h_full()
                .min_w_0()
                .overflow_hidden()
                .gap(theme.space.lg)
                .children(self.tabs.into_iter().enumerate().map(|(ix, tab)| {
                    let is_active = ix == active;
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
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
                                .children(tab.index_hint.map(|hint| Text::hint(hint).faint())),
                        )
                        .child(div().flex_none().h(underline_h).w_full().bg(if is_active {
                            theme.colors.accent
                        } else {
                            transparent_black()
                        }))
                }))
                .children((self.overflow > 0).then(|| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(theme.metrics.chip_h)
                        .child(Text::hint(format!("+{}", self.overflow)).faint())
                }))
                .into_any_element(),
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .size_full()
            .pl(self
                .leading_inset
                .unwrap_or(theme.metrics.traffic_light_inset))
            .pr(theme.space.md)
            .gap(theme.space.md)
            .bg(theme.colors.bg)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(left)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    // §2.2: 8 px between chips, 12 px from the daemon dot.
                    .gap(theme.space.sm)
                    .children(self.chips)
                    .child(div().flex().flex_none().pl(theme.space.xs).child(dot)),
            )
    }
}
