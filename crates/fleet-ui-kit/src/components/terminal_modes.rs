//! `TerminalModes` — the small badges for the VT modes a `FrameUpdate` reports.
//!
//! `FrameUpdate.modes` is the only thing that explains why the terminal stopped behaving the
//! way the keymap says it does: in alt-screen there is no scrollback (`ctrl-s [` refuses),
//! with mouse reporting on the app owns drag-select, and without bracketed paste `ctrl-s ]`
//! is unsafe in an editor. Each badge is **zero-suppressed** — a plain shell shows none of
//! them, so the row costs nothing in the common case.

use gpui::{App, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// One VT mode worth telling the user about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TerminalMode {
    /// The alternate screen buffer is active: there is no scrollback.
    AltScreen,
    /// The app is reading mouse events, so drag-select belongs to it.
    MouseReporting,
    /// The app asked for bracketed paste, so `ctrl-s ]` wraps the payload.
    BracketedPaste,
    /// The app switched the cursor keys to application mode.
    ApplicationCursor,
}

impl TerminalMode {
    /// The short badge word.
    pub fn label(self) -> &'static str {
        match self {
            TerminalMode::AltScreen => "alt",
            TerminalMode::MouseReporting => "mouse",
            TerminalMode::BracketedPaste => "paste",
            TerminalMode::ApplicationCursor => "appcur",
        }
    }

    /// The glyph, taken from the closed icon set.
    pub fn icon(self) -> Icon {
        match self {
            TerminalMode::AltScreen => Icon::Maximize2,
            TerminalMode::MouseReporting => Icon::Eye,
            TerminalMode::BracketedPaste => Icon::ClipboardCheck,
            TerminalMode::ApplicationCursor => Icon::Command,
        }
    }

    /// `alt` is amber because it changes what a documented key (`ctrl-s [`) does; the rest are
    /// muted facts.
    pub fn tone(self) -> Tone {
        match self {
            TerminalMode::AltScreen => Tone::Warning,
            _ => Tone::Muted,
        }
    }
}

/// A zero-suppressed row of mode badges.
#[derive(IntoElement)]
pub struct TerminalModes {
    modes: Vec<TerminalMode>,
    labelled: bool,
}

impl TerminalModes {
    /// A row over the active modes. Order is normalized so the row never reshuffles between
    /// frames.
    pub fn new(modes: impl IntoIterator<Item = TerminalMode>) -> Self {
        let mut modes: Vec<_> = modes.into_iter().collect();
        modes.sort();
        modes.dedup();
        Self {
            modes,
            labelled: true,
        }
    }

    /// Drop the words and keep only the glyphs, for a narrow header.
    pub fn glyphs_only(mut self) -> Self {
        self.labelled = false;
        self
    }

    /// Whether the row draws anything.
    pub fn is_visible(&self) -> bool {
        !self.modes.is_empty()
    }

    /// Whether the alt-screen badge is present — the caller uses the same fact to suppress
    /// [`ScrollPill`](crate::components::ScrollPill).
    pub fn is_alt_screen(&self) -> bool {
        self.modes.contains(&TerminalMode::AltScreen)
    }
}

impl RenderOnce for TerminalModes {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.is_visible() {
            return div().into_any_element();
        }
        let theme = cx.theme().clone();
        let labelled = self.labelled;
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .children(self.modes.into_iter().map(move |mode| {
                let color = mode.tone().color(&theme);
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.xxs)
                    .h(theme.metrics.chip_h)
                    .px(theme.space.xs)
                    .rounded(theme.radii.full)
                    .bg(mode.tone().fill(&theme))
                    .child(mode.icon().el().size(IconSize::Small).color(color))
                    .children(labelled.then(|| Text::hint(mode.label()).color(color)))
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_modes_are_zero_suppressed() {
        assert!(!TerminalModes::new([]).is_visible());
    }

    #[test]
    fn modes_are_sorted_and_deduped() {
        let m = TerminalModes::new([
            TerminalMode::BracketedPaste,
            TerminalMode::AltScreen,
            TerminalMode::AltScreen,
        ]);
        assert_eq!(
            m.modes,
            vec![TerminalMode::AltScreen, TerminalMode::BracketedPaste]
        );
        assert!(m.is_alt_screen());
    }
}
