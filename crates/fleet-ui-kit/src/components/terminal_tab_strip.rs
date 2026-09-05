//! `TerminalTabStrip` — numbered tabs 84-200 px with activity, keep-alive and exit marks.
//!
//! §3.6: the index is the argument to `ctrl-s 1`-`9`, so the strip is the legend for that
//! binding. The 6 px amber activity dot is the only background-activity signal in the app.
//!
//! The strip is a **legend**, not a control surface: every tab is reachable from the keyboard
//! without it. The click handlers ([`TerminalTabStrip::on_select`],
//! [`TerminalTabStrip::on_new`]) exist for mouse parity only, which is why they are optional
//! and why nothing here draws a close button — `^s x` is the way a tab is closed, and a hover
//! target that kills a running dev server is not worth the pixel.

use gpui::{App, ElementId, SharedString, Window, div, prelude::*, px};

use crate::{
    components::{Spinner, StatusDot},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The narrowest a tab may get before its name is ellipsized (§3.6).
const TAB_MIN_W: f32 = 84.0;
/// The widest a tab may get, so eight terminals still fit on a 1280 px window.
const TAB_MAX_W: f32 = 200.0;
/// The trailing `+` tab.
const NEW_TAB_W: f32 = 36.0;

/// One terminal tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalTab {
    /// The `1`-`9` index.
    pub index: usize,
    /// The tab name (`nvim`, `cc`, `lg`).
    pub name: SharedString,
    /// Output happened since this tab was last visited.
    pub activity: bool,
    /// The PTY is still spawning: §3.6 "Waking a slept session" rebuilds the strip and each
    /// tab shows a `loader-circle` until its process is up.
    pub starting: bool,
    /// The keep-alive kind glyph: `bot`, `server`, `file-pen`.
    pub keep_alive: Option<Icon>,
    /// The command exited. `Some(None)` is a signal-killed process, which has **no** exit
    /// code; the strip renders `—` rather than inventing one.
    pub exited: Option<Option<i32>>,
}

impl TerminalTab {
    /// A tab.
    pub fn new(index: usize, name: impl Into<SharedString>) -> Self {
        Self {
            index,
            name: name.into(),
            activity: false,
            starting: false,
            keep_alive: None,
            exited: None,
        }
    }

    /// Mark unread output.
    pub fn activity(mut self, activity: bool) -> Self {
        self.activity = activity;
        self
    }

    /// Mark the PTY as still spawning.
    pub fn starting(mut self, starting: bool) -> Self {
        self.starting = starting;
        self
    }

    /// Mark a keep-alive process.
    pub fn keep_alive(mut self, icon: Icon) -> Self {
        self.keep_alive = Some(icon);
        self
    }

    /// Mark the command as exited. Takes `1` or `None`: a signal-killed process has no code.
    pub fn exited(mut self, code: impl Into<Option<i32>>) -> Self {
        self.exited = Some(code.into());
        self
    }

    /// The exit code as it is written on the tab: the number, or `—` for a signal.
    fn exit_label(code: Option<i32>) -> SharedString {
        match code {
            Some(code) => SharedString::from(code.to_string()),
            None => SharedString::new_static("\u{2014}"),
        }
    }
}

/// The tab strip.
#[derive(IntoElement)]
pub struct TerminalTabStrip {
    id: ElementId,
    tabs: Vec<TerminalTab>,
    active: usize,
    show_plus: bool,
    #[allow(clippy::type_complexity)]
    on_select: Option<std::rc::Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>>,
    #[allow(clippy::type_complexity)]
    on_new: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl TerminalTabStrip {
    /// A strip over the session's terminals.
    pub fn new(tabs: impl IntoIterator<Item = TerminalTab>) -> Self {
        Self {
            id: ElementId::Name(SharedString::new_static("terminal-tab-strip")),
            tabs: tabs.into_iter().collect(),
            active: 0,
            show_plus: true,
            on_select: None,
            on_new: None,
        }
    }

    /// A stable id, so two strips in one window keep their hover and animation state apart.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
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

    /// Mouse parity for `ctrl-s 1`-`9`: called with the clicked tab's **position**.
    pub fn on_select(mut self, on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(on_select));
        self
    }

    /// Mouse parity for `ctrl-s c`, fired by the trailing `+` tab.
    pub fn on_new(mut self, on_new: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_new = Some(Box::new(on_new));
        self
    }
}

impl RenderOnce for TerminalTabStrip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active;
        let strip_id = self.id;
        let on_select = self.on_select;
        let on_new = self.on_new;
        let tab_theme = theme.clone();
        let tab_strip_id = strip_id.clone();

        let tabs =
            self.tabs.into_iter().enumerate().map(move |(pos, tab)| {
                let theme = &tab_theme;
                let strip_id = &tab_strip_id;
                let is_active = pos == active;
                let exited = tab.exited;
                let starting = tab.starting;
                let index = tab.index;
                let select = on_select.clone();
                let hover_bg = theme.colors.row_hover;

                div()
                    .id(ElementId::NamedChild(
                        std::sync::Arc::new(strip_id.clone()),
                        SharedString::from(format!("tab-{pos}")),
                    ))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .flex_none()
                    .min_w(px(TAB_MIN_W))
                    .max_w(px(TAB_MAX_W))
                    .h_full()
                    .border_r(px(1.0))
                    .border_color(theme.colors.border)
                    // The active tab sits one step above the strip so the eye finds "where am I"
                    // before it reads any name.
                    .when(is_active, |el| el.bg(theme.colors.surface))
                    .when(!is_active, |el| el.hover(move |s| s.bg(hover_bg)))
                    .when_some(select, |el, select| {
                        el.cursor_pointer()
                            .on_click(move |_, window, cx| select(pos, window, cx))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(theme.space.xs)
                            .px(theme.space.sm)
                            // The index is the argument to `ctrl-s <n>`; it never dims away,
                            // because the moment it does the binding stops being discoverable.
                            .child(Text::hint(index.to_string()).faint())
                            .child(
                                if exited.is_some() {
                                    Text::ui(tab.name).faint()
                                } else if is_active {
                                    Text::ui_strong(tab.name)
                                } else {
                                    Text::ui(tab.name).muted()
                                }
                                .ellipsize(),
                            )
                            .children(starting.then(|| {
                                Spinner::new(ElementId::NamedChild(
                                    std::sync::Arc::new(strip_id.clone()),
                                    SharedString::from(format!("tab-{pos}-starting")),
                                ))
                                .size(IconSize::Small)
                            }))
                            .children(tab.keep_alive.map(|icon| {
                                icon.el()
                                    .size(IconSize::Small)
                                    .color(theme.colors.text_secondary)
                            }))
                            .children(exited.map(|code| {
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(theme.space.xxs)
                                    .child(
                                        Icon::CircleX
                                            .el()
                                            .size(IconSize::Small)
                                            .color(theme.colors.text_muted),
                                    )
                                    .child(Text::hint(TerminalTab::exit_label(code)).faint())
                            }))
                            .children(
                                // Activity on the tab you are already looking at is not news.
                                (tab.activity && !is_active && exited.is_none())
                                    .then(|| StatusDot::small(Tone::Warning)),
                            ),
                    )
                    .child(div().flex_none().h(theme.metrics.focus_ring_w).w_full().bg(
                        if is_active {
                            theme.colors.accent
                        } else {
                            gpui::transparent_black()
                        },
                    ))
            });

        div()
            .flex()
            .items_stretch()
            .h(theme.metrics.pane_header_h)
            .w_full()
            .bg(theme.colors.bg)
            .border_b(px(1.0))
            .border_color(theme.colors.border)
            .overflow_hidden()
            .children(tabs)
            .children(self.show_plus.then(|| {
                div()
                    .id(ElementId::NamedChild(
                        std::sync::Arc::new(strip_id.clone()),
                        SharedString::new_static("new"),
                    ))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(px(NEW_TAB_W))
                    .h_full()
                    .hover(|s| s.bg(theme.colors.row_hover))
                    .when_some(on_new, |el, on_new| {
                        el.cursor_pointer()
                            .on_click(move |_, window, cx| on_new(window, cx))
                    })
                    .child(
                        Icon::Plus
                            .el()
                            .size(IconSize::Medium)
                            .color(theme.colors.text_muted),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_killed_tab_shows_a_dash_and_never_a_number() {
        assert_eq!(TerminalTab::exit_label(Some(1)).as_ref(), "1");
        assert_eq!(TerminalTab::exit_label(None).as_ref(), "\u{2014}");
    }

    #[test]
    fn exited_accepts_both_a_code_and_none() {
        assert_eq!(TerminalTab::new(1, "cc").exited(1).exited, Some(Some(1)));
        assert_eq!(TerminalTab::new(1, "cc").exited(None).exited, Some(None));
    }
}
