//! The first-run page and pane-local empty-state copy.

use std::path::{Path, PathBuf};

use fleet_ui_kit::{
    ActiveTheme, Button, ButtonSize, ButtonStyle, DaemonDot, DaemonState, EmptyState,
    HarnessTargetExt, Icon, IconSize, StepCard, StepMark, Text, Tone, prelude::*,
};
use gpui::{AnyElement, App, SharedString, div};

use crate::actions::{filter, first_run as first_run_actions, fleet, hub, repos};

/// What Fleet is for, in one sentence (§3.13).
const HEADLINE: &str =
    "Run many branches and agents at once \u{2014} they keep running when you close Fleet.";
/// The line under the headline.
const SUBTITLE: &str = "Three steps to your first workspace.";

/// One numbered step of the first-run page: its title and the line under it.
struct Step {
    title: &'static str,
    description: &'static str,
}

/// The three steps, in order. The first two run an action; the third needs a repository.
const STEPS: [Step; 3] = [
    Step {
        title: "Create a context",
        description: "Group repositories by GitHub org or client, e.g. \u{201c}Acme\u{201d}.",
    },
    Step {
        title: "Clone a repository",
        description: "Search your orgs on GitHub; it clones in the background.",
    },
    Step {
        title: "Start a worktree and an agent",
        description: "A branch copy with its own terminals and Claude or Codex threads.",
    },
];
/// Why step 3 cannot run yet.
const STEP_3_NOTE: &str = "after step 2";
/// The import card, shown only when `~/.swarm/state.json` exists.
const IMPORT_TITLE: &str = "Import from ~/.swarm";
/// The reassurance that makes the import safe to click.
const IMPORT_IS_SAFE: &str =
    "Brings over contexts, repos and worktrees. Nothing in ~/.swarm changes.";

/// Typed copy for each empty surface. `{}` in a fact is replaced by the caller's scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmptySurface {
    Repos,
    Worktrees,
    WorktreesRepo,
    WorktreesNoRepos,
    Filter,
    PrsMine,
    PrsReview,
}

impl EmptySurface {
    pub(crate) fn copy(self, scope: Option<&str>) -> (SharedString, &'static str) {
        let (fact, action) = match self {
            Self::Repos => ("No repos in {}.", "n  clone one"),
            Self::Worktrees => ("No worktrees yet", "n  create one"),
            Self::WorktreesRepo => ("No worktrees for {} yet", "n  create one"),
            Self::WorktreesNoRepos => (
                "No repositories yet \u{2014} clone one to start a worktree",
                "",
            ),
            Self::Filter => ("Nothing matches \"{}\".", "Clear filter"),
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
        match self {
            // The Worktrees page is redesigned: its one way forward is a button that shows its
            // key, not a key line (ADR 0023).
            Self::Worktrees | Self::WorktreesRepo => EmptyState::new(fact)
                .button(crate::views::worktrees_list::new_worktree_button())
                .into_any_element(),
            // Step 2 of §3.13 once the first-run page has given way: nothing to branch from
            // yet, so the page's one way forward is the clone, as the primary.
            Self::WorktreesNoRepos => EmptyState::new(fact)
                .button(
                    Button::new(
                        "worktrees-empty-clone",
                        crate::views::worktrees_list::label(&repos::Clone),
                    )
                    .icon(Icon::Plus)
                    .style(ButtonStyle::Primary)
                    .action(Box::new(repos::Clone))
                    .harness_target("worktrees.empty.clone"),
                )
                .into_any_element(),
            // The sidebar says the same thing with a control: the button runs the key the old
            // line named, and shows it. (The PR screen draws its own Refresh button.)
            Self::Repos => EmptyState::new(fact)
                .button(
                    Button::new(
                        "repos-empty-clone",
                        crate::views::worktrees_list::label(&repos::Clone),
                    )
                    .action(Box::new(repos::Clone)),
                )
                .into_any_element(),
            // A filter miss offers the way out as a button: one click clears the query from
            // either stage of the two-stage `Esc`, and the chip is the `esc` that clears it once
            // the input has been left — none while typing, where `esc` only leaves the input.
            Self::Filter => EmptyState::new(fact)
                .button(
                    Button::new("filter-empty-clear", action)
                        .icon(Icon::X)
                        .action(Box::new(filter::Clear))
                        .key_of(Box::new(fleet::Cancel))
                        .harness_target("filter.empty.clear"),
                )
                .into_any_element(),
            Self::PrsMine | Self::PrsReview => {
                EmptyState::new(fact).action(action).into_any_element()
            }
        }
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

#[must_use]
fn footer_line(version: Option<&str>, home: &Path, user_home: Option<&Path>) -> String {
    let home_text = home.to_string_lossy();
    let home = crate::presentation::tilde(&home_text, user_home);
    match version {
        Some(version) => format!(
            "fleetd running \u{00b7} {} \u{00b7} {home}",
            crate::presentation::bare_version(version)
        ),
        None => format!("fleetd \u{00b7} {home}"),
    }
}

/// The first-run page of §3.13: what Fleet is for, three steps you can click, the import when
/// there is something to import, and a footer with the daemon, Help and Settings.
///
/// Every control dispatches the action its key runs in `FirstRun` and shows that key from the
/// live keymap, so the page is the keymap's own affordance rather than a copy of it.
#[must_use]
pub(crate) fn card_with_home(
    home: &Path,
    daemon_version: Option<&str>,
    has_swarm: bool,
    user_home: Option<&Path>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let tile = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(theme.metrics.alert_tile)
        .rounded(theme.radii.card)
        .bg(Tone::Accent.fill(theme))
        .child(
            Icon::Sailboat
                .el()
                .size(IconSize::Large)
                .color(Tone::Accent.color(theme)),
        );
    let header = div()
        .flex()
        .flex_col()
        .gap(theme.space.md)
        .child(tile)
        .child(Text::page_title(HEADLINE))
        .child(Text::ui(SUBTITLE).tone(Tone::Secondary));

    let [create, clone, start] = &STEPS;
    let steps = div()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        .child(
            StepCard::new("first-run-step-1", StepMark::Number(1), create.title)
                .description(create.description)
                .current(true)
                .action(Box::new(hub::NewContext))
                .harness_target_indexed("first_run.step", 0),
        )
        .child(
            StepCard::new("first-run-step-2", StepMark::Number(2), clone.title)
                .description(clone.description)
                .action(Box::new(repos::Clone))
                .harness_target_indexed("first_run.step", 1),
        )
        .child(
            StepCard::new("first-run-step-3", StepMark::Number(3), start.title)
                .description(start.description)
                .unavailable(STEP_3_NOTE)
                .harness_target_indexed("first_run.step", 2),
        );

    let import = has_swarm.then(|| {
        StepCard::new(
            "first-run-import",
            StepMark::Icon(Icon::Sailboat),
            IMPORT_TITLE,
        )
        .description(IMPORT_IS_SAFE)
        .dashed(true)
        .action(Box::new(first_run_actions::Import))
        .harness_target("first_run.import")
    });

    let footer = div()
        .flex()
        .items_center()
        .gap(theme.space.md)
        .pt(theme.space.sm)
        .border_t(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(theme.space.sm)
                .child(DaemonDot::new(DaemonState::Healthy))
                .child(
                    Text::caption(SharedString::from(footer_line(
                        daemon_version,
                        home,
                        user_home,
                    )))
                    .tone(Tone::Muted)
                    .ellipsize(),
                ),
        )
        .child(
            Button::new("first-run-help", "Keyboard shortcuts")
                .style(ButtonStyle::Ghost)
                .size(ButtonSize::Compact)
                .action(Box::new(fleet::OpenHelp))
                .harness_target("first_run.help"),
        )
        .child(
            Button::new("first-run-settings", "Settings")
                .style(ButtonStyle::Ghost)
                .size(ButtonSize::Compact)
                .action(Box::new(fleet::OpenSettings))
                .harness_target("first_run.settings"),
        );

    div()
        .flex()
        .items_center()
        .justify_center()
        .size_full()
        .child(
            div()
                .flex()
                .flex_col()
                .w(theme.metrics.first_run_w)
                .gap(theme.space.xl)
                .child(header)
                .child(steps)
                .children(import)
                .child(footer),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_are_the_three_the_page_promises() {
        assert!(SUBTITLE.starts_with("Three steps"));
        assert_eq!(STEPS.len(), 3);
        assert_eq!(STEPS[0].title, "Create a context");
        assert_eq!(STEPS[1].title, "Clone a repository");
        assert!(
            IMPORT_IS_SAFE.contains("Nothing in ~/.swarm changes"),
            "the import says it modifies nothing"
        );
    }

    #[test]
    fn scoped_copy_substitutes_only_the_fact() {
        assert_eq!(
            EmptySurface::WorktreesRepo.copy(Some("payroll")),
            ("No worktrees for payroll yet".into(), "n  create one")
        );
        assert_eq!(
            EmptySurface::Filter.copy(Some("rut")),
            ("Nothing matches \"rut\".".into(), "Clear filter")
        );
        assert_eq!(
            EmptySurface::Worktrees.copy(None),
            ("No worktrees yet".into(), "n  create one"),
            "an unscoped fact is a static string, not a substitution"
        );
    }

    #[test]
    fn the_footer_states_the_home_and_the_version() {
        let home = Path::new("/home/u/.fleet");
        let user = Path::new("/home/u");
        assert_eq!(
            footer_line(Some("fleetd 0.1.0"), home, Some(user)),
            "fleetd running \u{00b7} 0.1.0 \u{00b7} ~/.fleet",
            "the daemon's build string already says `fleetd`, and `~` is where it lives"
        );
        assert_eq!(
            footer_line(Some("0.1.0"), home, None),
            "fleetd running \u{00b7} 0.1.0 \u{00b7} /home/u/.fleet",
            "without a home to compare against, the path is spelled out"
        );
        assert_eq!(
            footer_line(None, home, Some(user)),
            "fleetd \u{00b7} ~/.fleet",
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
