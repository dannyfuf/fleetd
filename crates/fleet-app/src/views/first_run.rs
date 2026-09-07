//! First-run migration card and pane-local empty-state copy.

use std::path::{Path, PathBuf};

use fleet_ui_kit::{ActiveTheme, EmptyState, Icon, IconSize, Text, Tone, prelude::*};
use gpui::{AnyElement, App, SharedString, div, px};

/// The card's title.
const TITLE: &str = "Fleet";
/// The card's one-sentence promise, which is also the promise the Jobs panel demonstrates.
const TAGLINE: &str =
    "Copies, sessions and PRs — all owned by fleetd, so they survive this window.";
/// The sentence that appears only when there is something to migrate.
const FOUND_SWARM: &str = "Found ~/.swarm.";
/// The reassurance that makes `i` safe to press.
const IMPORT_IS_SAFE: &str = "(nothing in ~/.swarm is modified)";

/// Typed copy for each empty surface. `{}` in a fact is replaced by the caller's scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmptySurface {
    Contexts,
    Repos,
    Worktrees,
    WorktreesRepo,
    Filter,
    PrsMine,
    PrsReview,
}

impl EmptySurface {
    pub(crate) fn copy(self, scope: Option<&str>) -> (SharedString, &'static str) {
        let (fact, action) = match self {
            Self::Contexts => ("No contexts yet.", "N  create your first context"),
            Self::Repos => ("No repos in {}.", "n  clone one"),
            Self::Worktrees => ("No worktrees yet.", "n  create one"),
            Self::WorktreesRepo => ("No worktrees for {} yet.", "n  create one"),
            Self::Filter => ("Nothing matches \"{}\".", "esc  clear"),
            Self::PrsMine => ("No open PRs authored by you in {}.", "r  refresh"),
            Self::PrsReview => ("No PRs waiting for your review in {}.", "r  refresh"),
        };
        (
            if fact.contains("{}") {
                fact.replacen("{}", scope.unwrap_or_default(), 1).into()
            } else {
                SharedString::new_static(fact)
            },
            action,
        )
    }

    pub(crate) fn render(self, scope: Option<&str>) -> AnyElement {
        let (fact, action) = self.copy(scope);
        EmptyState::new(fact).action(action).into_any_element()
    }
}

/// `~/.swarm/state.json`, the file whose existence turns the first run into a migration.
#[must_use]
fn swarm_state_path(home: Option<&Path>) -> Option<PathBuf> {
    Some(home?.join(".swarm").join("state.json"))
}

/// Whether the import block is shown at all (§3.13, zero-suppression).
#[must_use]
pub fn has_swarm_state(home: Option<&Path>) -> bool {
    swarm_state_path(home).is_some_and(|path| path.exists())
}

/// First-run key alignment; no shared semantic token exists for this column.
const KEY_COLUMN: f32 = 20.0;

/// The keys the card offers, in spec order. `i` appears only with something to import.
#[must_use]
pub fn keys(has_swarm: bool) -> &'static [(&'static str, &'static str)] {
    if has_swarm { &KEYS } else { &KEYS[1..] }
}

const KEYS: [(&str, &str); 5] = [
    ("i", "import contexts, repos and worktrees"),
    ("N", "create your first context"),
    ("n", "clone a repository"),
    ("?", "keymap"),
    (",", "settings"),
];
const HINT_ROWS: &[&[(&str, &str)]] = &[&[KEYS[1]], &[KEYS[2]], &[KEYS[3], KEYS[4]]];

#[must_use]
fn footer_line(version: Option<&str>, home: &Path, user_home: Option<&Path>) -> String {
    let home_text = home.to_string_lossy();
    let home = crate::presentation::tilde(&home_text, user_home);
    match version {
        Some(version) => format!(
            "\u{25CD} fleetd running \u{00b7} {} \u{00b7} {home}",
            crate::presentation::bare_version(version)
        ),
        None => format!("\u{25CD} fleetd \u{00b7} {home}"),
    }
}

/// The single centered card of §3.13.
#[must_use]
pub(crate) fn card_with_home(
    home: &Path,
    daemon_version: Option<&str>,
    has_swarm: bool,
    user_home: Option<&Path>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let glyph = if has_swarm {
        Icon::Sailboat
    } else {
        Icon::Boxes
    };

    let hint = |key: &'static str, label: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .child(div().w(px(KEY_COLUMN)).flex_none().child(Text::hint(key)))
            .child(Text::hint(label))
    };

    let keys = keys(has_swarm);
    let import_block = has_swarm.then(|| {
        let (key, label) = keys[0];
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(theme.space.xs)
            .child(Text::ui(FOUND_SWARM))
            .child(hint(key, label))
            .child(
                div()
                    .pl(px(KEY_COLUMN) + theme.space.sm)
                    .child(Text::hint(IMPORT_IS_SAFE).faint()),
            )
    });

    let hints = div()
        .flex()
        .flex_col()
        .items_start()
        .gap(theme.space.xs)
        .children(HINT_ROWS.iter().map(|row| {
            div()
                .flex()
                .items_center()
                .gap(theme.space.lg)
                .children(row.iter().map(|(key, label)| hint(key, label)))
        }));

    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .gap(theme.space.lg)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    glyph
                        .el()
                        .size(IconSize::Large)
                        .color(theme.colors.text_secondary),
                )
                .child(Text::title(TITLE)),
        )
        .child(Text::ui(TAGLINE).muted())
        .children(import_block)
        .child(hints)
        .child(
            Text::hint(SharedString::from(footer_line(
                daemon_version,
                home,
                user_home,
            )))
            .tone(Tone::Muted),
        )
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
    fn the_hint_rows_use_the_non_import_keys_in_order() {
        let rows: Vec<_> = HINT_ROWS
            .iter()
            .flat_map(|row| row.iter().copied())
            .collect();
        assert_eq!(rows, keys(false));
    }

    #[test]
    fn scoped_copy_substitutes_only_the_fact() {
        assert_eq!(
            EmptySurface::WorktreesRepo.copy(Some("payroll")),
            ("No worktrees for payroll yet.".into(), "n  create one")
        );
        assert_eq!(
            EmptySurface::Filter.copy(Some("rut")),
            ("Nothing matches \"rut\".".into(), "esc  clear")
        );
        assert_eq!(
            EmptySurface::Contexts.copy(None),
            ("No contexts yet.".into(), "N  create your first context"),
            "an unscoped fact is a static string, not a substitution"
        );
    }

    #[test]
    fn the_footer_states_the_home_and_the_version() {
        let home = Path::new("/home/u/.fleet");
        let user = Path::new("/home/u");
        assert_eq!(
            footer_line(Some("fleetd 0.1.0"), home, Some(user)),
            "\u{25CD} fleetd running \u{00b7} 0.1.0 \u{00b7} ~/.fleet",
            "the daemon's build string already says `fleetd`, and `~` is where it lives"
        );
        assert_eq!(
            footer_line(Some("0.1.0"), home, None),
            "\u{25CD} fleetd running \u{00b7} 0.1.0 \u{00b7} /home/u/.fleet",
            "without a home to compare against, the path is spelled out"
        );
        assert_eq!(
            footer_line(None, home, Some(user)),
            "\u{25CD} fleetd \u{00b7} ~/.fleet",
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
