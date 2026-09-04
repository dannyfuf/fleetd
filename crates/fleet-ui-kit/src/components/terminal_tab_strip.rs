//! `TerminalTabStrip` — numbered tabs 84-200 px with activity, keep-alive and exit marks.
//!
//! §3.6: the index is the argument to `ctrl-s 1`-`9`, so the strip is the legend for that
//! binding. The 6 px amber activity dot is the only background-activity signal in the app.

use gpui::{App, SharedString, Window, div, prelude::*, px};

use crate::{
    components::StatusDot,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// One terminal tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalTab {
    /// The `1`-`9` index.
    pub index: usize,
    /// The tab name (`nvim`, `cc`, `lg`).
    pub name: SharedString,
    /// Output happened since this tab was last visited.
    pub activity: bool,
    /// The keep-alive kind glyph: `bot`, `server`, `file-pen`.
    pub keep_alive: Option<Icon>,
    /// The command exited with this code. The label drops to the faintest tone.
    pub exited: Option<i32>,
}

impl TerminalTab {
    /// A tab.
    pub fn new(index: usize, name: impl Into<SharedString>) -> Self {
        Self {
            index,
            name: name.into(),
            activity: false,
            keep_alive: None,
            exited: None,
        }
    }

    /// Mark unread output.
    pub fn activity(mut self, activity: bool) -> Self {
        self.activity = activity;
        self
    }

    /// Mark a keep-alive process.
    pub fn keep_alive(mut self, icon: Icon) -> Self {
        self.keep_alive = Some(icon);
        self
    }

    /// Mark the command as exited.
    pub fn exited(mut self, code: i32) -> Self {
        self.exited = Some(code);
        self
    }
}

/// The tab strip.
#[derive(IntoElement)]
pub struct TerminalTabStrip {
    tabs: Vec<TerminalTab>,
    active: usize,
    show_plus: bool,
}

impl TerminalTabStrip {
    /// A strip over the session's terminals.
    pub fn new(tabs: impl IntoIterator<Item = TerminalTab>) -> Self {
        Self {
            tabs: tabs.into_iter().collect(),
            active: 0,
            show_plus: true,
        }
    }

    /// Which tab index (position, not `Terminal.index`) is active.
    pub fn active(mut self, active: usize) -> Self {
        self.active = active;
        self
    }

    /// Hide the trailing `+` tab.
    pub fn show_plus(mut self, show: bool) -> Self {
        self.show_plus = show;
        self
    }
}

impl RenderOnce for TerminalTabStrip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active;
        div()
            .flex()
            .items_stretch()
            .h(theme.metrics.pane_header_h)
            .w_full()
            .bg(theme.colors.bg)
            .border_b(px(1.0))
            .border_color(theme.colors.border)
            .children(self.tabs.into_iter().enumerate().map(|(pos, tab)| {
                let is_active = pos == active;
                let exited = tab.exited;
                div()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .min_w(px(84.0))
                    .max_w(px(200.0))
                    .border_r(px(1.0))
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .gap(theme.space.xs)
                            .px(theme.space.sm)
                            .child(Text::hint(tab.index.to_string()).faint())
                            .child(if exited.is_some() {
                                Text::ui(tab.name).faint()
                            } else if is_active {
                                Text::ui_strong(tab.name)
                            } else {
                                Text::ui(tab.name).muted()
                            })
                            .children(
                                tab.keep_alive.map(|icon| {
                                    icon.el()
                                        .size(IconSize::Small)
                                        .color(theme.colors.text_secondary)
                                }),
                            )
                            .children(exited.map(|code| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space.xxs)
                                    .child(
                                        Icon::CircleX
                                            .el()
                                            .size(IconSize::Small)
                                            .color(theme.colors.text_muted),
                                    )
                                    .child(Text::hint(code.to_string()).faint())
                            }))
                            .children(
                                (tab.activity && !is_active)
                                    .then(|| StatusDot::small(Tone::Warning)),
                            ),
                    )
                    .child(div().h(theme.metrics.focus_ring_w).w_full().bg(
                        if is_active {
                            theme.colors.accent
                        } else {
                            gpui::transparent_black()
                        },
                    ))
            }))
            .children(self.show_plus.then(|| {
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(36.0))
                    .child(
                        Icon::Plus
                            .el()
                            .size(IconSize::Medium)
                            .color(theme.colors.text_muted),
                    )
            }))
    }
}
