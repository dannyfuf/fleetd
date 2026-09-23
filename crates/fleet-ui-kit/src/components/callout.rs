//! `Callout` — a tinted box inside a dialog body that states a consequence before the user
//! commits: "Prepared copy ready — about 2 s", or "No prepared copy — the first create copies
//! the repo (~40 s) in the background".
//!
//! One tone, one glyph, one line of text, and an optional muted detail line under it. It is
//! information, not an alarm: a [`super::Banner`] spans a screen and says something is wrong
//! now, and a [`super::Dialog`]'s footer error says the last action failed. A callout says
//! what *will* happen.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A tinted note about what an action will do.
#[derive(IntoElement)]
pub struct Callout {
    tone: Tone,
    icon: Icon,
    text: SharedString,
    detail: Option<SharedString>,
}

impl Callout {
    /// A callout in `tone`, led by `icon`.
    pub fn new(tone: Tone, icon: Icon, text: impl Into<SharedString>) -> Self {
        Self {
            tone,
            icon,
            text: text.into(),
            detail: None,
        }
    }

    /// A muted second line: what else happens (`Hooks: pnpm install (run in background)`).
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl RenderOnce for Callout {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = self.tone.color(theme);
        div()
            .flex()
            .w_full()
            .gap(theme.space.sm)
            .px(theme.space.md)
            .py(theme.space.sm)
            .rounded(theme.radii.md)
            .bg(self.tone.fill(theme))
            .border(theme.metrics.hairline)
            .border_color(color.opacity(theme.metrics.banner_border_opacity))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(theme.text.ui.line_height)
                    .items_center()
                    .child(self.icon.el().size(IconSize::Medium).color(color)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(Text::ui(self.text))
                    .children(self.detail.map(|detail| Text::ui(detail).muted())),
            )
    }
}
