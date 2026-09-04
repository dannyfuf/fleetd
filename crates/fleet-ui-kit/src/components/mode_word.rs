//! `ModeWord` — the fixed 84 px word in the center of the status bar.
//!
//! §1.9 and §2.8: the app is modal, and a visible word prevents the single most expensive
//! mistake in a modal app — typing a command into a PTY, or a PTY key into a list. It is
//! present on every screen, including the Workspace and including zoom.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// The eight modes of the app, and their words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Lists. `NORMAL`.
    Normal,
    /// Keys go to the PTY. `TERMINAL`.
    Terminal,
    /// One-shot after `ctrl-s`. `^S`, amber.
    Prefix,
    /// Scrollback / copy mode. `SCROLL`.
    Scroll,
    /// Filter input. `FILTER`.
    Filter,
    /// Command palette. `PALETTE`.
    Palette,
    /// Any dialog. `DIALOG`.
    Dialog,
    /// Jobs panel. `JOBS`.
    Jobs,
}

impl Mode {
    /// The word rendered in the status bar.
    pub fn word(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Terminal => "TERMINAL",
            Mode::Prefix => "^S",
            Mode::Scroll => "SCROLL",
            Mode::Filter => "FILTER",
            Mode::Palette => "PALETTE",
            Mode::Dialog => "DIALOG",
            Mode::Jobs => "JOBS",
        }
    }

    /// The tone. Only `Prefix` is amber, because it is the one mode that expires on its own.
    pub fn tone(self) -> Tone {
        match self {
            Mode::Prefix => Tone::Warning,
            _ => Tone::Secondary,
        }
    }
}

/// The mode word, at its fixed width.
#[derive(IntoElement)]
pub struct ModeWord {
    word: SharedString,
    tone: Tone,
}

impl ModeWord {
    /// From a known mode.
    pub fn new(mode: Mode) -> Self {
        Self {
            word: SharedString::new_static(mode.word()),
            tone: mode.tone(),
        }
    }

    /// From a literal word, for a mode the kit does not know about.
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
            .child(Text::label(self.word).tone(self.tone))
    }
}
