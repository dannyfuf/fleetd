//! `Avatar` — a person as a round badge of their initials.
//!
//! A card tile draws its assignee this way, and the card detail draws every comment's author
//! the same way, so a name reads as the same mark on both surfaces. The kit takes the name as a
//! string and derives the initials itself ([`super::initials`]); which tone a person wears is the
//! caller's choice.

use gpui::{App, SharedString, Window, div, prelude::*};

use super::card_tile::initials;
use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// A round `avatar_size` badge of a name's initials on its tone's fill.
#[derive(IntoElement)]
pub struct Avatar {
    initials: SharedString,
    tone: Tone,
}

impl Avatar {
    /// The avatar of `name`, a person's name or handle, in the accent tone.
    pub fn new(name: &str) -> Self {
        Self {
            initials: SharedString::from(initials(name)),
            tone: Tone::Accent,
        }
    }

    /// Set the tone: the letters take its colour and the disc its fill.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(theme.metrics.avatar_size)
            .rounded(theme.radii.pill)
            .bg(self.tone.fill(theme))
            .child(Text::caption(self.initials).tone(self.tone))
    }
}
