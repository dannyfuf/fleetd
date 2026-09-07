//! Bounded sticky-error text and screen-specific focus hints.

use fleet_ui_kit::StickyErrorSlot;
use gpui::{AnyElement, IntoElement, SharedString};

use crate::state::{Screen, StickyError};

/// The key that focuses the slot from the current screen.
///
/// `KEYMAP.md` binds `!` in `Hub` and `ctrl-s !` in `Workspace > Prefix`; the affordance drawn
/// over a terminal grid must spell the chord it needs, not the one the Hub uses.
#[must_use]
pub(crate) const fn focus_key(screen: &Screen) -> &'static str {
    match screen {
        Screen::Hub { .. } => "!",
        Screen::Workspace { .. } => "^s !",
    }
}

/// How much of an error the 26 px status bar can carry before the mode word is at risk.
const MAX_LINE: usize = 72;

/// One line of at most `budget` characters, so a long `gh` error cannot push the mode word out
/// of the status bar. Truncation is at the tail, because the head of an error names its cause.
#[must_use]
fn one_line(text: &str, budget: usize) -> String {
    let mut words = text.split_whitespace();
    let mut characters = words
        .by_ref()
        .flat_map(|word| std::iter::once(' ').chain(word.chars()))
        .skip(1);
    let mut line: String = characters.by_ref().take(budget).collect();
    if characters.next().is_some() {
        line.pop();
        line.push('…');
    }
    line
}

/// The status-bar slot itself.
#[must_use]
pub(crate) fn render(error: &StickyError, screen: &Screen) -> AnyElement {
    StickyErrorSlot::new(SharedString::from(one_line(&error.text, MAX_LINE)))
        .key(focus_key(screen))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use fleet_core::ids::SessionId;

    use super::*;
    use crate::state::HubTab;

    #[test]
    fn the_focus_key_carries_the_prefix_over_a_terminal() {
        let hub = Screen::Hub {
            tab: HubTab::Worktrees,
        };
        assert_eq!(focus_key(&hub), "!");
        let workspace = Screen::Workspace {
            session: SessionId::try_from("payroll/feat-rut")
                .unwrap_or_else(|error| panic!("{error}")),
        };
        assert_eq!(
            focus_key(&workspace),
            "^s !",
            "a bare `!` over a terminal would go to the PTY"
        );
    }

    #[test]
    fn a_long_error_is_collapsed_to_one_line() {
        let long = "gh: HTTP 502\n  upstream connect error or disconnect/reset before headers";
        let line = one_line(long, 40);
        assert!(!line.contains('\n'));
        assert_eq!(line.chars().count(), 40);
        assert!(line.ends_with('\u{2026}'));
        assert_eq!(one_line("short", 40), "short");
    }
}
