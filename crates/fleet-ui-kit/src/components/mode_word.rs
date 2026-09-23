//! `ModeWord` — the fixed 84 px word at the right of the embedded Git UI's status bar.
//!
//! Fleet's own chrome draws no mode word (ADR 0023): a state that changes what keys do is shown
//! on the surface that has it. The embedded Git UI (`fleet-lazygit`) keeps one, because it mirrors
//! lazygit, whose status line names the key owner (`NORMAL`, `STAGING`, `DIALOG`) and the
//! repository operation (`REBASING`, `MERGING`).
//!
//! The 84 px is fixed rather than intrinsic on purpose: words of different lengths would move
//! whatever sits beside the word every time the key owner changes.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// The mode word, at its fixed width.
#[derive(IntoElement)]
pub struct ModeWord {
    word: SharedString,
    tone: Tone,
}

impl ModeWord {
    /// A word in the secondary tone.
    pub fn word(word: impl Into<SharedString>) -> Self {
        Self {
            word: word.into(),
            tone: Tone::Secondary,
        }
    }

    /// Override the tone.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

impl RenderOnce for ModeWord {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(theme.metrics.mode_word_w)
            .overflow_hidden()
            .child(Text::label(self.word).tone(self.tone))
    }
}
