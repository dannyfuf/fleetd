//! Bounded sticky-error text, and the status-bar slot that opens and dismisses it.

use fleet_ui_kit::{HarnessTargetExt, Kbd, StickyErrorSlot};
use gpui::{AnyElement, IntoElement, SharedString};

use crate::{actions::fleet, state::StickyError};

/// How much of an error the 28 px status bar can carry before it crowds out the breadcrumb.
const MAX_LINE: usize = 72;

/// One line of at most `budget` characters, so a long `gh` error cannot push the breadcrumb out
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
///
/// A click on the error runs `!` (`fleet::FocusStickyError`: the Jobs panel, on the failure) and
/// the ✕ beside it `fleet::DismissStickyError`, both dispatched as their keys would be. `kbd` is
/// the chip for `!` on the current screen — prefixed over a terminal, where a bare `!` would go to
/// the PTY (KEYMAP A18, [D-8]).
#[must_use]
pub(crate) fn render(error: &StickyError, kbd: Option<Kbd>) -> AnyElement {
    // `docs/TESTING-HARNESS.md` §3 names the error `sticky_error.retry`: the whole slot, which is
    // what a scenario clicks to open the failure and reads to prove the error is still up. The
    // slot names its own ✕ `sticky_error.close`.
    StickyErrorSlot::new(
        "sticky-error",
        SharedString::from(one_line(&error.text, MAX_LINE)),
    )
    .kbd(kbd)
    .action(Box::new(fleet::FocusStickyError))
    .dismiss_action(Box::new(fleet::DismissStickyError))
    .harness_target("sticky_error.retry")
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
