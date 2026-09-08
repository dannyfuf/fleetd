//! `ModeWord` — the fixed 84 px word in the center of the status bar.
//!
//! §1.9 and §2.8: Fleet is modal, and a visible word prevents the single most expensive
//! mistake a modal app can produce — typing a command into a PTY, or a PTY key into a list.
//! The word is present on **every** screen, including the Workspace and including zoom
//! (`ctrl-s z`).
//!
//! The 84 px is fixed rather than intrinsic on purpose: `NORMAL` and `TERMINAL` differ by four
//! characters, and a mode word that resizes moves the job ticker next to it every time the
//! user enters a terminal. A fixed slot means the eye can park there.
//!
//! ## States
//!
//! One per [`Mode`]. Only `Prefix` is amber, because it is the one mode that expires on its
//! own — everything else is amber-free by §1.4, since a mode is not a health signal.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// The nine modes of the app, and their words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// Lists. `NORMAL`.
    #[default]
    Normal,
    /// Keys go to the PTY. `TERMINAL`.
    Terminal,
    /// A native agent thread has the keyboard. `AGENT`.
    ///
    /// Distinct from [`Mode::Terminal`] because keys reach Fleet's own composer, not a PTY:
    /// the transcript answers `j`/`k` and the decision keys, and `^s` still prefixes.
    Agent,
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
    /// Every mode, in the order §2.8 lists them. Used by the gallery.
    pub const ALL: &'static [Mode] = &[
        Mode::Normal,
        Mode::Terminal,
        Mode::Agent,
        Mode::Prefix,
        Mode::Scroll,
        Mode::Filter,
        Mode::Palette,
        Mode::Dialog,
        Mode::Jobs,
    ];

    /// The word rendered in the status bar.
    pub const fn word(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Terminal => "TERMINAL",
            Mode::Agent => "AGENT",
            Mode::Prefix => "^S",
            Mode::Scroll => "SCROLL",
            Mode::Filter => "FILTER",
            Mode::Palette => "PALETTE",
            Mode::Dialog => "DIALOG",
            Mode::Jobs => "JOBS",
        }
    }

    /// The tone. Only `Prefix` is amber, because it is the one mode that expires on its own.
    pub const fn tone(self) -> Tone {
        match self {
            Mode::Prefix => Tone::Warning,
            _ => Tone::Secondary,
        }
    }

    /// Whether keys typed on this screen reach a PTY instead of the app. A view uses it to
    /// decide whether its own hints need the `^s` prefix (§3.6 [D-8]).
    pub const fn keys_reach_pty(self) -> bool {
        matches!(self, Mode::Terminal)
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
            .overflow_hidden()
            .child(Text::label(self.word).tone(self.tone))
    }
}
