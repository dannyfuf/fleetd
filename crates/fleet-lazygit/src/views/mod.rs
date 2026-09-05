//! Presentation: pure functions from git data to elements.
//!
//! Every rendering decision — glyph, tone, column text — is a free function over a `&FileStatus`,
//! `&Branch` or `&Commit`, so it can be unit tested without opening a window.

pub mod diff;
pub mod diff_model;
pub mod file_tree;
pub mod intraline;
pub mod rows;
pub mod syntax;

use fleet_ui_kit::Theme;
use gpui::Hsla;

/// lazygit names ANSI colours in its presentation code (`style.FgGreen`, `style.FgRed`, …). The
/// faithful mapping is the theme's terminal palette, not the semantic tokens, which stay reserved
/// for Fleet-level state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ansi {
    /// `style.FgRed` — unstaged, unpushed, removed, destructive.
    Red,
    /// `style.FgGreen` — staged, merged, added.
    Green,
    /// `style.FgYellow` — partially staged, pushed, pending.
    Yellow,
    /// `style.FgBlue` — neutral metadata, rebase in flight.
    Blue,
    /// `style.FgMagenta` — the thing you singled out.
    Magenta,
    /// `style.FgCyan` — navigational, informational, in progress.
    Cyan,
}

impl Ansi {
    /// The resolved colour for the installed theme.
    #[must_use]
    pub fn color(self, theme: &Theme) -> Hsla {
        let index = match self {
            Ansi::Red => 1,
            Ansi::Green => 2,
            Ansi::Yellow => 3,
            Ansi::Blue => 4,
            Ansi::Magenta => 5,
            Ansi::Cyan => 6,
        };
        theme.terminal.ansi[index]
    }
}
