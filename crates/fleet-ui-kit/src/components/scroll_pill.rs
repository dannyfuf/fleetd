//! `ScrollPill` — `SCROLL <offset>/<len>`, suppressed in alt-screen — and
//! [`ScrollbackBadge`], its always-on miniature.
//!
//! §3.6: 176 x 22 px, top-right **inside** the terminal area with a 12 px inset, because
//! during scroll the eyes are on content and the top right never covers the prompt. When an
//! alt-screen app is running the pill is suppressed entirely and `ctrl-s [` shows the 1.6 s
//! toast `no scrollback in alt-screen` instead.
//!
//! The two components here are not variants of each other. The **pill** is a *mode*
//! affordance: it exists while Scroll mode is active and it lists the keys that mode adds. The
//! **badge** is a *state* affordance: a viewport scrolled up with the wheel is not in Scroll
//! mode and would otherwise look exactly like a live one — which is how "my agent stopped
//! printing" bug reports are born.

use gpui::{App, Window, div, prelude::*, px};

use crate::{
    components::KeyHintRow,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The pill's fixed width (§3.6). Fixed, so the number changing does not resize the overlay
/// under the reader's eye.
const PILL_W: f32 = 176.0;
/// The amber left bar that marks the pill as a mode.
const MODE_BAR_W: f32 = 2.0;

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
            // Top-right, 12 px inside the terminal area: during scroll the eyes are on
            // content, and this corner never covers the prompt.
            .absolute()
            .top(theme.space.md)
            .right(theme.space.md)
            .flex()
            .flex_col()
            .justify_center()
            .gap(theme.space.xxs)
            .w(px(PILL_W))
            .min_h(theme.metrics.strip_h)
            .px(theme.space.sm)
            .py(theme.space.xxs)
            .rounded(theme.radii.md)
            // The floating layer, not the pane layer: the pill sits over the cell grid and has
            // to stay legible on top of whatever the shell just painted.
            .bg(theme.colors.elevated)
            .shadow(theme.sheet_shadow())
            .border_l(px(MODE_BAR_W))
            .border_color(theme.colors.warning)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.xs)
                    .child(
                        Icon::ChevronsUp
                            .el()
                            .size(IconSize::Small)
                            .color(theme.colors.warning),
                    )
                    .child(
                        Text::label(format!("SCROLL {}/{}", self.offset, self.scrollback_len))
                            .tone(Tone::Secondary),
                    ),
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
/// Zero-suppressed at `offset == 0` and in alt-screen, and
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
            .top(theme.space.md)
            .right(theme.space.md)
            .flex()
            .items_center()
            .gap(theme.space.xxs)
            .h(theme.metrics.chip_h)
            .px(theme.space.xs)
            .rounded(theme.radii.full)
            .bg(theme.colors.elevated)
            .shadow(theme.sheet_shadow())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_screen_suppresses_both_overlays() {
        assert!(ScrollPill::new(412, 2000).is_visible());
        assert!(!ScrollPill::new(412, 2000).alt_screen(true).is_visible());
        assert!(ScrollbackBadge::new(412, 2000).is_visible());
        assert!(
            !ScrollbackBadge::new(412, 2000)
                .alt_screen(true)
                .is_visible()
        );
    }

    #[test]
    fn a_live_viewport_shows_no_badge() {
        assert!(!ScrollbackBadge::new(0, 2000).is_visible());
    }

    #[test]
    fn the_pill_stays_visible_at_offset_zero() {
        // Scroll mode is entered at the bottom of the scrollback; a pill that only appeared
        // after the first `k` would make the mode look like it failed to engage.
        assert!(ScrollPill::new(0, 2000).is_visible());
    }
}
