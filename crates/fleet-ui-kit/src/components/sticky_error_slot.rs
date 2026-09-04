//! `StickyErrorSlot` — red, addressable with `!`, persists until dismissed.
//!
//! §1.8: errors are sticky and actionable; successes are transient. This slot owns the last
//! failed job, replaces the job ticker while present, and never decays on a timer.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    components::KeyHint,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// `⚠ gh: HTTP 502 · !`
#[derive(IntoElement)]
pub struct StickyErrorSlot {
    text: SharedString,
    key: SharedString,
}

impl StickyErrorSlot {
    /// An error with the default `!` focus key.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            key: SharedString::new_static("!"),
        }
    }

    /// Override the key that focuses the error.
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = key.into();
        self
    }
}

impl RenderOnce for StickyErrorSlot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = theme.colors.danger;
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .min_w_0()
            .child(Icon::TriangleAlert.el().size(IconSize::Small).color(color))
            .child(Text::ui(self.text).tone(Tone::Danger).ellipsize())
            .child(KeyHint::new(self.key))
    }
}
