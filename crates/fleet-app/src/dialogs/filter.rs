//! §3.10 Filter bar (`/`) — *narrow this list without moving it*.

use fleet_core::{ids::WorktreeId, model::Worktree};
use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::filter as filter_actions,
    bridge::Bridge,
    dialogs::{open_worktree, typed_char},
    state::{AppState, HubPane, HubTab, RepoScope, Screen},
};

/// The worktrees the Hub shows: scoped to the selected repo, then filtered.
#[must_use]
fn visible_worktrees(state: &AppState) -> Vec<&Worktree> {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return Vec::new();
    };
    let query = state.filter.query.to_ascii_lowercase();
    snapshot
        .worktrees
        .iter()
        .filter(|worktree| match &state.scope {
            RepoScope::All => true,
            RepoScope::Repo(repo) => &worktree.repo_id == repo,
        })
        .filter(|worktree| worktree.id.as_str().to_ascii_lowercase().contains(&query))
        .collect()
}

/// How many rows the focused list shows, and how many it has in total (`2/12`).
#[must_use]
fn counts(state: &AppState) -> (usize, usize) {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return (0, 0);
    };
    match (state.hub_pane, &state.screen) {
        (HubPane::Repos, _) => {
            let total = snapshot.repos.len();
            let query = state.filter.query.to_ascii_lowercase();
            let shown = snapshot
                .repos
                .iter()
                .filter(|repo| repo.id.as_str().to_ascii_lowercase().contains(&query))
                .count();
            (shown, total)
        }
        (
            HubPane::List,
            Screen::Hub {
                tab: HubTab::Worktrees,
            },
        ) => {
            let total = snapshot
                .worktrees
                .iter()
                .filter(|worktree| match &state.scope {
                    RepoScope::All => true,
                    RepoScope::Repo(repo) => &worktree.repo_id == repo,
                })
                .count();
            (visible_worktrees(state).len(), total)
        }
        _ => (0, 0),
    }
}

/// The header row while the input owns the keyboard (§3.10, line 2 of the mock).
#[must_use]
pub fn bar(state: &AppState) -> FilterBar {
    let (shown, total) = counts(state);
    FilterBar::new(state.filter.query.clone(), shown, total).focused(state.filter.editing)
}

/// The invisible element that owns Filter mode's keyboard.
///
/// It draws nothing: the bar itself lives in the pane header, which is the whole point of
/// §3.10 ("no overlay, no reflow"). The shell renders this in the overlay layer so the
/// `Filter` key context really does shadow the list behind it.
pub fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    _cx: &mut App,
) -> AnyElement {
    let accept_state = state.clone();
    let accept_bridge = bridge.clone();
    div()
        .track_focus(focus)
        .size_full()
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let Some(text) = typed_char(event) else {
                    return;
                };
                state.update(cx, |app, cx| {
                    app.filter.query.push_str(text);
                    app.cursors.worktrees = 0;
                    app.cursors.repos = 0;
                    cx.notify();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &filter_actions::Backspace, _window, cx| {
                state.update(cx, |app, cx| {
                    app.filter.query.pop();
                    cx.notify();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &filter_actions::DeleteWord, _window, cx| {
                state.update(cx, |app, cx| {
                    app.filter.query = delete_word(&app.filter.query);
                    cx.notify();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &filter_actions::Clear, _window, cx| {
                state.update(cx, |app, cx| {
                    app.filter.query.clear();
                    cx.notify();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &filter_actions::CursorDown, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &filter_actions::CursorUp, _window, cx| move_cursor(&state, -1, cx)
        })
        .on_action(move |_: &filter_actions::Accept, _window, cx| {
            accept(&accept_state, &accept_bridge, cx);
        })
        .into_any_element()
}

/// `ctrl-w`: drop trailing spaces, then the word before the caret.
#[must_use]
pub fn delete_word(query: &str) -> String {
    let trimmed = query.trim_end();
    match trimmed.rfind(char::is_whitespace) {
        Some(index) => {
            let end = trimmed[index..]
                .chars()
                .next()
                .map_or(index, |ch| index + ch.len_utf8());
            trimmed[..end].to_owned()
        }
        None => String::new(),
    }
}

/// `ctrl-n` / `ctrl-p`: move the **list** cursor while the input still owns the keyboard.
fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    state.update(cx, |app, cx| {
        let (shown, _) = counts(app);
        match app.hub_pane {
            HubPane::Repos => {
                app.cursors.repos = crate::state::move_cursor(app.cursors.repos, delta, shown);
            }
            HubPane::List => {
                app.cursors.worktrees =
                    crate::state::move_cursor(app.cursors.worktrees, delta, shown);
            }
        }
        cx.notify();
    });
}

/// `Enter`: open the highlighted row straight from the input, so `/rut⏎` is a complete open.
fn accept(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let target = {
        let app = state.read(cx);
        let query = app.filter.query.to_ascii_lowercase();
        match app.hub_pane {
            HubPane::Repos => app
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .repos
                        .iter()
                        .filter(|repo| repo.id.as_str().to_ascii_lowercase().contains(&query))
                        .nth(app.cursors.repos)
                })
                .map(|repo| Target::Repo(repo.id.clone())),
            HubPane::List => visible_worktrees(app)
                .get(app.cursors.worktrees)
                .map(|worktree| Target::Worktree(worktree.id.clone())),
        }
    };
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
    match target {
        Target::Repo(repo) => {
            state.update(cx, |app, cx| {
                app.scope = RepoScope::Repo(repo);
                app.hub_pane = HubPane::List;
                cx.notify();
            });
        }
        Target::Worktree(id) => open_worktree(id, state, bridge, cx),
    }
}

/// What `Enter` opens, resolved before the state is mutated.
enum Target {
    Repo(fleet_core::ids::RepoId),
    Worktree(WorktreeId),
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn delete_word_eats_the_trailing_word_only() {
        assert_eq!(delete_word("feat rut "), "feat ");
        assert_eq!(delete_word("feat\u{2003}rut "), "feat\u{2003}");
        assert_eq!(delete_word("feat"), "");
        assert_eq!(delete_word(""), "");
    }

    #[test]
    fn counts_are_zero_without_a_snapshot() {
        let state = AppState::new("/tmp/fleet", Instant::now());
        assert_eq!(counts(&state), (0, 0));
        assert!(visible_worktrees(&state).is_empty());
    }
}
