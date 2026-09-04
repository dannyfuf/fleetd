//! The first-run card of UX-SPEC §3.13, and every empty state the app can show.
//!
//! **[D-18]: for this user the empty state is a migration, not an onboarding.** The import row
//! is the primary path and is shown *only* when `~/.swarm/state.json` exists; without it the
//! block is omitted entirely (zero-suppression, §1.2) and the card falls back to `N` / `n` /
//! `?` under a single `boxes` glyph. There is no carousel, tour, checklist or sample data.
//!
//! Copy is swarm's, verbatim: [`EMPTY_STATES`] is the §3.13 table as data, so a screen renders
//! the sanctioned wording instead of inventing its own.

use std::path::{Path, PathBuf};

use fleet_ui_kit::{
    ActiveTheme, EmptyState, Icon, IconSize, KeyHint, KeyHintRow, Text, Tone, prelude::*,
};
use gpui::{AnyElement, App, SharedString, div};

/// The card's title.
pub const TITLE: &str = "Fleet";
/// The card's one-sentence promise, which is also the promise the Jobs panel demonstrates.
pub const TAGLINE: &str =
    "Copies, sessions and PRs — all owned by fleetd, so they survive this window.";
/// The sentence that appears only when there is something to migrate.
pub const FOUND_SWARM: &str = "Found ~/.swarm.";
/// The reassurance that makes `i` safe to press.
pub const IMPORT_IS_SAFE: &str = "(nothing in ~/.swarm is modified)";

/// One row of the §3.13 empty-state table: the fact, then the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyStateCopy {
    /// The surface the wording belongs to.
    pub surface: &'static str,
    /// Line 1: the fact. `{}` is substituted with the scope when the row has one.
    pub fact: &'static str,
    /// Line 2, faint: the key.
    pub action: &'static str,
}

/// The §3.13 table, verbatim. Every empty state in the app renders from this.
pub const EMPTY_STATES: &[EmptyStateCopy] = &[
    EmptyStateCopy {
        surface: "contexts",
        fact: "No contexts yet.",
        action: "N  create your first context",
    },
    EmptyStateCopy {
        surface: "repos",
        fact: "No repos in {}.",
        action: "n  clone one",
    },
    EmptyStateCopy {
        surface: "worktrees",
        fact: "No worktrees yet.",
        action: "n  create one",
    },
    EmptyStateCopy {
        surface: "worktrees-repo",
        fact: "No worktrees for {} yet.",
        action: "n  create one",
    },
    EmptyStateCopy {
        surface: "filter",
        fact: "Nothing matches \"{}\".",
        action: "esc  clear",
    },
    EmptyStateCopy {
        surface: "prs-mine",
        fact: "No open PRs authored by you in {}.",
        action: "r  refresh",
    },
    EmptyStateCopy {
        surface: "prs-review",
        fact: "No PRs waiting for your review in {}.",
        action: "r  refresh",
    },
    EmptyStateCopy {
        surface: "jobs",
        fact: "Nothing running.",
        action: "Jobs and sessions live in fleetd, so they survive closing this window.",
    },
    EmptyStateCopy {
        surface: "terminal-exited",
        fact: "process exited ({})",
        action: "^s x  close    ^s c  new    ^s r  restart",
    },
];

/// Looks a §3.13 row up by surface, substituting the one `{}` placeholder when it has one.
///
/// Returns `None` for an unknown surface rather than inventing wording: a screen that needs a
/// new empty state adds a row to [`EMPTY_STATES`] and to `docs/UX-SPEC.md`, in that order.
#[must_use]
pub fn empty_copy(surface: &str, scope: Option<&str>) -> Option<(String, String)> {
    let row = EMPTY_STATES.iter().find(|row| row.surface == surface)?;
    let fact = match scope {
        Some(scope) => row.fact.replacen("{}", scope, 1),
        None => row.fact.replace("{}", ""),
    };
    Some((fact, row.action.to_owned()))
}

/// Renders one §3.13 empty state, **inside the affected pane only** — never full-screen, so the
/// surrounding panes stay usable.
#[must_use]
pub fn empty_state(surface: &str, scope: Option<&str>) -> AnyElement {
    match empty_copy(surface, scope) {
        Some((fact, action)) => EmptyState::new(fact).action(action).into_any_element(),
        None => div().into_any_element(),
    }
}

/// `~/.swarm/state.json`, the file whose existence turns the first run into a migration.
#[must_use]
pub fn swarm_state_path(home: Option<&Path>) -> Option<PathBuf> {
    Some(home?.join(".swarm").join("state.json"))
}

/// Whether the import block is shown at all (§3.13, zero-suppression).
#[must_use]
pub fn has_swarm_state(home: Option<&Path>) -> bool {
    swarm_state_path(home).is_some_and(|path| path.exists())
}

/// The user's home directory.
#[must_use]
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The keys the card offers, in spec order. `i` appears only with something to import.
#[must_use]
pub fn keys(has_swarm: bool) -> Vec<(&'static str, &'static str)> {
    let mut keys = Vec::with_capacity(5);
    if has_swarm {
        keys.push(("i", "import contexts, repos and worktrees"));
    }
    keys.push(("N", "create your first context"));
    keys.push(("n", "clone a repository"));
    keys.push(("?", "keymap"));
    keys.push((",", "settings"));
    keys
}

/// The card's footer: `◍ fleetd <version> · <home>`, the two facts that answer "is it there?".
#[must_use]
pub fn footer_line(version: Option<&str>, home: &Path) -> String {
    match version {
        Some(version) => format!(
            "\u{25CD} fleetd running \u{00b7} {version} \u{00b7} {}",
            home.display()
        ),
        None => format!("\u{25CD} fleetd \u{00b7} {}", home.display()),
    }
}

/// The single centered card of §3.13.
#[must_use]
pub fn card(
    home: &Path,
    daemon_version: Option<&str>,
    has_swarm: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let glyph = if has_swarm { Icon::Sailboat } else { Icon::Boxes };

    let import_block = has_swarm.then(|| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(theme.space.xs)
            .child(Text::ui(FOUND_SWARM))
            .child(KeyHint::labeled("i", "import contexts, repos and worktrees"))
            .child(Text::hint(IMPORT_IS_SAFE).faint())
    });

    // `i` is drawn by the import block above, with the sentence that makes it safe to press.
    let mut hints = KeyHintRow::new();
    for (key, label) in keys(has_swarm).into_iter().filter(|(key, _)| *key != "i") {
        hints = hints.key(key, label);
    }

    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .gap(theme.space.lg)
        .child(
            glyph
                .el()
                .size(IconSize::Large)
                .color(theme.colors.text_secondary),
        )
        .child(Text::title(TITLE))
        .child(Text::ui(TAGLINE).muted())
        .children(import_block)
        .child(hints)
        .child(Text::hint(SharedString::from(footer_line(
            daemon_version,
            home,
        )))
        .tone(Tone::Muted))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_import_row_is_zero_suppressed() {
        let with_swarm = keys(true);
        assert_eq!(with_swarm.first().map(|row| row.0), Some("i"));
        assert_eq!(with_swarm.len(), 5);

        let without = keys(false);
        assert!(
            !without.iter().any(|row| row.0 == "i"),
            "without ~/.swarm the import block is omitted entirely"
        );
        assert_eq!(without.len(), 4);
    }

    #[test]
    fn the_card_never_offers_a_tour() {
        let text = format!("{TITLE} {TAGLINE} {FOUND_SWARM} {IMPORT_IS_SAFE}");
        for banned in ["tour", "carousel", "checklist", "sample", "welcome"] {
            assert!(
                !text.to_lowercase().contains(banned),
                "§3.13 forbids `{banned}` in the first-run card"
            );
        }
        assert!(IMPORT_IS_SAFE.contains("nothing in ~/.swarm is modified"));
    }

    #[test]
    fn every_documented_empty_state_is_available_verbatim() {
        assert_eq!(
            empty_copy("worktrees", None),
            Some(("No worktrees yet.".to_owned(), "n  create one".to_owned()))
        );
        assert_eq!(
            empty_copy("worktrees-repo", Some("payroll")),
            Some((
                "No worktrees for payroll yet.".to_owned(),
                "n  create one".to_owned()
            ))
        );
        assert_eq!(
            empty_copy("filter", Some("rut")),
            Some((
                "Nothing matches \"rut\".".to_owned(),
                "esc  clear".to_owned()
            ))
        );
        assert_eq!(
            empty_copy("jobs", None).map(|copy| copy.0),
            Some("Nothing running.".to_owned())
        );
        assert_eq!(empty_copy("no-such-surface", None), None);
    }

    #[test]
    fn the_exited_terminal_state_spells_its_prefix() {
        let (fact, action) =
            empty_copy("terminal-exited", Some("130")).unwrap_or_else(|| panic!("missing row"));
        assert_eq!(fact, "process exited (130)");
        assert!(action.starts_with("^s x"), "no bare keys over a terminal");
    }

    #[test]
    fn the_footer_states_the_home_and_the_version() {
        let home = Path::new("/home/u/.fleet");
        assert_eq!(
            footer_line(Some("0.1.0"), home),
            "\u{25CD} fleetd running \u{00b7} 0.1.0 \u{00b7} /home/u/.fleet"
        );
        assert_eq!(
            footer_line(None, home),
            "\u{25CD} fleetd \u{00b7} /home/u/.fleet",
            "an unknown version is omitted, never guessed"
        );
    }

    #[test]
    fn the_swarm_probe_needs_a_home() {
        assert_eq!(swarm_state_path(None), None);
        assert!(!has_swarm_state(None));
        assert_eq!(
            swarm_state_path(Some(Path::new("/home/u"))),
            Some(PathBuf::from("/home/u/.swarm/state.json"))
        );
    }
}
