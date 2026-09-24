//! `DegradedChip` — `⚠ hooks failed`.
//!
//! §3.3: a worktree that looks ready but whose post-create hooks failed is a trap. This chip
//! occupies the same row slot as [`super::KeepAliveChips`] and **outranks** it.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The degraded marker.
#[derive(IntoElement)]
pub struct DegradedChip {
    text: SharedString,
}

impl DegradedChip {
    /// The default wording, `hooks failed`.
    pub fn hooks_failed() -> Self {
        Self::new("hooks failed")
    }

    /// A degraded marker with custom wording.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for DegradedChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = theme.colors.warning;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .child(Icon::TriangleAlert.el().size(IconSize::Medium).color(color))
            .child(Text::ui(self.text).tone(Tone::Warning))
    }
}
