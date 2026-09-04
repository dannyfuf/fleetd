//! `ScrollPill` — `SCROLL <offset>/<len>`, suppressed in alt-screen — and
//! [`ScrollbackBadge`], its always-on miniature.
//!
//! §3.6: 176 x 22 px, top-right **inside** the terminal area with a 12 px inset, because
//! during scroll the eyes are on content and the top right never covers the prompt. When an
//! alt-screen app is running the pill is suppressed entirely and `ctrl-s [` shows the 1.6 s
//! toast `no scrollback in alt-screen` instead.

use gpui::{App, Window, div, prelude::*, px};

use crate::{
    components::KeyHintRow,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The scroll-mode overlay.
#[derive(IntoElement)]
pub struct ScrollPill {
    offset: usize,
    scrollback_len: usize,
    selecting: bool,
    alt_screen: bool,
}

impl ScrollPill {
    /// A pill for the current viewport.
    pub fn new(offset: usize, scrollback_len: usize) -> Self {
        Self {
            offset,
            scrollback_len,
            selecting: false,
            alt_screen: false,
        }
    }

    /// Add the second line `v select · y yank · Esc exit`.
    pub fn selecting(mut self, selecting: bool) -> Self {
        self.selecting = selecting;
        self
    }

    /// Suppress the pill because an alt-screen app is running.
    pub fn alt_screen(mut self, alt_screen: bool) -> Self {
        self.alt_screen = alt_screen;
        self
    }

    /// Whether the pill draws anything.
    pub fn is_visible(&self) -> bool {
        !self.alt_screen
    }
}

impl RenderOnce for ScrollPill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.is_visible() {
            return div().into_any_element();
        }
        let theme = cx.theme();
        div()
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .flex()
            .flex_col()
            .w(px(176.0))
            .min_h(theme.metrics.strip_h)
            .px(theme.space.sm)
            .py(gpui::px(2.0))
            .rounded(theme.radii.md)
            .bg(theme.colors.surface)
            .border_l(gpui::px(2.0))
            .border_color(theme.colors.warning)
            .child(
                Text::label(format!("SCROLL {}/{}", self.offset, self.scrollback_len))
                    .tone(Tone::Secondary),
            )
            .children(self.selecting.then(|| {
                KeyHintRow::new()
                    .key("v", "select")
                    .key("y", "yank")
                    .key("Esc", "exit")
            }))
            .into_any_element()
    }
}

/// `↥ <offset>/<len>` — the scrollback-offset badge.
///
/// The pill above is a *mode* affordance and only exists while Scroll mode is active. This
/// badge is the *state* affordance: a viewport scrolled up by a mouse wheel is not in Scroll
/// mode and would otherwise look exactly like a live one, which is how "my agent stopped
/// printing" bug reports are born. It is zero-suppressed at `offset == 0` and
/// [`TerminalGrid::scrollback`](crate::components::TerminalGrid::scrollback) paints it for
/// free.
#[derive(IntoElement)]
pub struct ScrollbackBadge {
    offset: usize,
    scrollback_len: usize,
    alt_screen: bool,
}

impl ScrollbackBadge {
    /// A badge for `viewport { offset, scrollback_len }`.
    pub fn new(offset: usize, scrollback_len: usize) -> Self {
        Self {
            offset,
            scrollback_len,
            alt_screen: false,
        }
    }

    /// Suppress the badge because an alt-screen app is running (there is no scrollback).
    pub fn alt_screen(mut self, alt_screen: bool) -> Self {
        self.alt_screen = alt_screen;
        self
    }

    /// Whether the badge draws anything: only a scrolled-back, non-alt-screen viewport.
    pub fn is_visible(&self) -> bool {
        self.offset > 0 && !self.alt_screen
    }
}

impl RenderOnce for ScrollbackBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.is_visible() {
            return div().into_any_element();
        }
        let theme = cx.theme();
        div()
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .flex()
            .items_center()
            .gap(theme.space.xxs)
            .h(theme.metrics.chip_h)
            .px(theme.space.xs)
            .rounded(theme.radii.full)
            .bg(theme.colors.surface)
            .child(
                Icon::ChevronsUp
                    .el()
                    .size(IconSize::Small)
                    .color(theme.colors.warning),
            )
            .child(
                Text::hint(format!("{}/{}", self.offset, self.scrollback_len))
                    .tone(Tone::Secondary),
            )
            .into_any_element()
    }
}
