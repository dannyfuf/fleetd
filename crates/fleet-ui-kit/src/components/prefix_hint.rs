//! `PrefixHint` — the `^S` pill and its six keys, 400 ms after the prefix.
//!
//! §3.6: the expert types the second key in under 200 ms and never sees this; the returning
//! user gets it exactly when they hesitate. That is 0 px and 0 frames of permanent cost, which
//! is why the delay is part of the contract and not a preference.
//!
//! **Minimal render.** The 400 ms delay is the caller's timer (`cx.spawn` + a
//! `Timer::after(theme.motion.prefix_hint_delay)`); this component renders when told to.

use gpui::{App, SharedString, Window, div, prelude::*, px};

use crate::{
    components::KeyHintRow, text::Text, theme::ActiveTheme, tone::Tone,
};

/// The prefix cheat strip.
#[derive(IntoElement)]
pub struct PrefixHint {
    visible: bool,
    prefix: SharedString,
    hints: KeyHintRow,
}

impl PrefixHint {
    /// A hint strip. `visible` is false until the caller's 400 ms timer fires.
    pub fn new(visible: bool) -> Self {
        Self {
            visible,
            prefix: SharedString::new_static("^S"),
            hints: KeyHintRow::new(),
        }
    }

    /// Override the prefix pill's label.
    pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// The six most-used prefix keys.
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.hints = hints;
        self
    }
}

impl RenderOnce for PrefixHint {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let theme = cx.theme();
        div()
            .absolute()
            .left(px(12.0))
            .bottom(px(12.0))
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .px(theme.space.sm)
            .py(gpui::px(2.0))
            .rounded(theme.radii.md)
            .bg(theme.colors.surface)
            .border_1()
            .border_color(theme.colors.border)
            .child(
                div()
                    .px(theme.space.xs)
                    .rounded(theme.radii.sm)
                    .bg(Tone::Warning.fill(theme))
                    .child(Text::hint(self.prefix).tone(Tone::Warning)),
            )
            .child(self.hints)
            .into_any_element()
    }
}
