//! §3.10 Filter bar (`/`) — *narrow this list without moving it*.
//!
//! The query is a live [`TextInput`] the Hub screen owns, so this module decodes no editing
//! keys at all: `backspace`, `ctrl-w` and `ctrl-u` are rows of the `FleetTextInput` table
//! (`docs/KEYMAP.md`). What stays here is everything the *container* owns while the input has
//! the keyboard — the list cursor, `Enter` and the two-stage `Esc`.

use fleet_ui_kit::{TextInput, prelude::*};
use gpui::{App, Div, Entity};

use crate::{
    actions::filter as filter_actions,
    presentation::{filter_counts, filter_target},
    screens::hub::HubCtx,
    state::AppState,
};

#[cfg(test)]
use crate::{
    presentation::{DisplayedPr, DisplayedTarget},
    state::{HubPane, HubTab, RepoScope, Screen},
};

/// How many rows the focused list shows, and how many it has in total (`2/12`).
#[must_use]
fn counts(state: &AppState) -> (usize, usize) {
    filter_counts(state)
}

/// The header row while the input owns the keyboard (§3.10, line 2 of the mock).
#[must_use]
pub fn bar(state: &AppState, input: Entity<TextInput>) -> FilterBar {
    let (shown, total) = counts(state);
    FilterBar::new(input, shown, total)
}

/// Attaches the keys Filter mode's **container** owns to the Hub body.
///
/// The bar is drawn in the pane header, which is part of the body, so the focused editor and
/// these listeners are on one dispatch path — the same arrangement the board screen uses.
pub(crate) fn key_owner(element: Div, state: &Entity<AppState>, hub: HubCtx) -> Div {
    let accept_state = state.clone();
    let accept_hub = hub.clone();
    let down_hub = hub.clone();
    element
        // §3.10's `ctrl-n` / `↓` is the list's own movement, not a second implementation of it:
        // it must move the anchored row too, or the next projection snaps the cursor back.
        .on_action(move |_: &filter_actions::CursorDown, window, cx| {
            down_hub.move_by(1, window, cx);
        })
        .on_action(move |_: &filter_actions::CursorUp, window, cx| {
            hub.move_by(-1, window, cx);
        })
        .on_action(move |_: &filter_actions::Accept, _window, cx| {
            accept(&accept_state, &accept_hub, cx);
        })
}

/// `Enter`: open the highlighted row straight from the input, so `/rut⏎` is a complete open.
fn accept(state: &Entity<AppState>, hub: &HubCtx, cx: &mut App) {
    let target = filter_target(state.read(cx));
    let Some(target) = target else {
        // §3.10: with no match, `Enter` is inert.
        return;
    };
    state.update(cx, |app, cx| {
        app.close_overlay();
        // §3.10: a filter does not survive a screen change, and opening is one.
        app.filter = crate::state::FilterState::default();
        cx.notify();
    });
    hub.activate_filter_target(target, cx);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::screens::hub::tests::test_hub_ctx_for;

    fn displayed_repo(
        kind: crate::presentation::DisplayedRepoKind,
        repo: Option<&str>,
        job: Option<&str>,
    ) -> crate::presentation::DisplayedRepo {
        crate::presentation::DisplayedRepo {
            kind,
            repo: repo.map(|id| id.parse().expect("repo id")),
            job: job.map(|id| id.parse().expect("job id")),
        }
    }

    #[test]
    fn counts_are_zero_without_a_snapshot() {
        let state = AppState::new("/tmp/fleet", Instant::now());
        assert_eq!(counts(&state), (0, 0));
    }

    #[test]
    fn filter_targets_all_and_pull_requests_from_displayed_rows() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.displayed_hub.repos = vec![
            displayed_repo(crate::presentation::DisplayedRepoKind::All, None, None),
            displayed_repo(
                crate::presentation::DisplayedRepoKind::Repo,
                Some("acme/api"),
                None,
            ),
        ];
        state.displayed_hub.repo_total = 3;
        state.hub_pane = HubPane::Repos;
        assert_eq!(counts(&state), (2, 3));
        assert_eq!(filter_target(&state), Some(DisplayedTarget::AllRepos));

        state.hub_pane = HubPane::List;
        state.screen = Screen::Hub { tab: HubTab::Prs };
        state.displayed_hub.prs = vec![DisplayedPr {
            repo: "acme/api".parse().expect("repo id"),
            number: 42,
            local: Some("acme/api#topic".parse().expect("worktree id")),
        }];
        state.displayed_hub.pr_total = 4;
        assert_eq!(counts(&state), (1, 4));
        assert!(matches!(
            filter_target(&state),
            Some(DisplayedTarget::PullRequest(DisplayedPr { number: 42, .. }))
        ));
    }

    #[gpui::test]
    fn filter_accepts_clone_failed_like_hub(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet", Instant::now());
            state.hub_pane = HubPane::Repos;
            state.overlay = Some(crate::state::Overlay::Filter);
            state.filter = crate::state::FilterState {
                query: "acme".to_owned(),
                editing: true,
            };
            state.displayed_hub.repos = vec![displayed_repo(
                crate::presentation::DisplayedRepoKind::CloneFailed,
                Some("acme/api"),
                Some("clone-job"),
            )];
            state
        });
        let (hub, _harness) = test_hub_ctx_for(state.clone(), cx);

        cx.update(|cx| accept(&state, &hub, cx));

        cx.read(|cx| {
            let app = state.read(cx);
            assert!(matches!(app.overlay, Some(crate::state::Overlay::Jobs)));
            assert_eq!(
                app.jobs_focus.as_ref().map(|job| job.as_str()),
                Some("clone-job")
            );
            // §3.10: a filter does not survive a screen change, and opening is one.
            assert_eq!(app.filter, crate::state::FilterState::default());
        });
    }

    #[gpui::test]
    fn filter_repo_activation_resets_worktree_cursor(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet", Instant::now());
            state.hub_pane = HubPane::Repos;
            state.overlay = Some(crate::state::Overlay::Filter);
            state.filter = crate::state::FilterState {
                query: "acme".to_owned(),
                editing: true,
            };
            state.cursors.worktrees = 7;
            state.displayed_hub.repos = vec![displayed_repo(
                crate::presentation::DisplayedRepoKind::Repo,
                Some("acme/api"),
                None,
            )];
            state
        });
        let (hub, _harness) = test_hub_ctx_for(state.clone(), cx);

        cx.update(|cx| accept(&state, &hub, cx));

        cx.read(|cx| {
            let app = state.read(cx);
            assert_eq!(
                app.scope,
                RepoScope::Repo("acme/api".parse().expect("repo id"))
            );
            assert_eq!(app.cursors.worktrees, 0);
            assert_eq!(app.hub_pane, HubPane::List);
            assert!(app.overlay.is_none());
            assert_eq!(app.filter, crate::state::FilterState::default());
        });
    }
}
