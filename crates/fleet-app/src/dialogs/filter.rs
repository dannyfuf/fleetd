//! §3.10 Filter bar (`/`) — *narrow this list without moving it*.
//!
//! The filter **replaces the pane header in place**: 30 px, same row, no overlay, no reflow.
//! Two pieces make that work and they live in different places:
//!
//! * [`bar`] and [`retained_chip`] are what the focused pane draws in its header row, so the
//!   list that owns the header owns the pixels;
//! * [`render`] is the invisible keyboard host the shell puts in the overlay layer while
//!   `Overlay::Filter` is open — it carries the typing, the two `ctrl-n`/`ctrl-p` keys that
//!   move the **list** cursor while you are still typing, and the `Enter` that opens the
//!   highlighted row.
//!
//! **[D-15]**: `Esc` in the Hub never quits. The two stages — leave the input keeping the
//! filter, then clear it — are [`crate::state::filter_escape`] and belong to the shell.

use fleet_core::{ids::WorktreeId, model::Worktree};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::filter as filter_actions,
    bridge::Bridge,
    dialogs::{notify, typed_char},
    state::{AppState, HubPane, HubTab, RepoScope, Screen},
};

/// Whether a row survives the filter.
///
/// The match is a case-insensitive substring, not a subsequence: a list filter that hides rows
/// you can see the letters of is worse than one that asks for the letters in order.
#[must_use]
pub fn matches(haystack: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    haystack
        .to_ascii_lowercase()
        .contains(&query.to_ascii_lowercase())
}

/// The worktrees the Hub shows: scoped to the selected repo, then filtered.
#[must_use]
pub fn visible_worktrees(state: &AppState) -> Vec<&Worktree> {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return Vec::new();
    };
    snapshot
        .worktrees
        .iter()
        .filter(|worktree| match &state.scope {
            RepoScope::All => true,
            RepoScope::Repo(repo) => &worktree.repo_id == repo,
        })
        .filter(|worktree| matches(worktree.id.as_str(), &state.filter.query))
        .collect()
}

/// How many rows the focused list shows, and how many it has in total (`2/12`).
#[must_use]
pub fn counts(state: &AppState) -> (usize, usize) {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return (0, 0);
    };
    match (state.hub_pane, &state.screen) {
        (HubPane::Repos, _) => {
            let total = snapshot.repos.len();
            let shown = snapshot
                .repos
                .iter()
                .filter(|repo| matches(repo.id.as_str(), &state.filter.query))
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

/// The `⌕rut` chip a restored pane header carries while a filter is retained (§3.10, line 3).
///
/// A hidden active filter is the classic "where did my rows go" bug, so the chip is not
/// optional: draw it whenever [`crate::state::FilterState::is_active`] and the input is gone.
#[must_use]
pub fn retained_chip(state: &AppState) -> Option<Chip> {
    (state.filter.is_active() && !state.filter.editing)
        .then(|| Chip::labeled(Icon::Search, state.filter.query.clone()).tone(Tone::Accent))
}

/// The empty-result body: `Nothing matches "<filter>".` plus `esc clear` (§3.10 States).
#[must_use]
pub fn empty_state(state: &AppState) -> EmptyState {
    EmptyState::new(format!("Nothing matches \"{}\".", state.filter.query)).action("esc  clear")
}

// ---------------------------------------------------------------------------- keyboard host

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
                    app.filter.query.push_str(&text);
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
        Some(index) => trimmed[..=index].to_owned(),
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
    notify(state, cx);
}

/// `Enter`: open the highlighted row straight from the input, so `/rut⏎` is a complete open.
fn accept(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let target = {
        let app = state.read(cx);
        match app.hub_pane {
            HubPane::Repos => app
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .repos
                        .iter()
                        .filter(|repo| matches(repo.id.as_str(), &app.filter.query))
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

fn open_worktree(id: WorktreeId, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let reply = bridge.request(RequestBody::EnsureSession {
        worktree: Some(id),
        agent: None,
        sleep_previous: true,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::Session(session))) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            state.update(cx, |app, cx| {
                app.touch_session(session.id.clone());
                app.screen = Screen::Workspace {
                    session: session.id.clone(),
                };
                cx.notify();
            });
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crate::state::{FilterEscape, FilterState, filter_escape};

    use super::*;

    #[test]
    fn matching_is_case_insensitive_and_substring() {
        assert!(matches("buk/payroll#fix-RUT", "rut"));
        assert!(matches("anything", ""));
        assert!(!matches("buk/payroll", "zzz"));
    }

    #[test]
    fn delete_word_eats_the_trailing_word_only() {
        assert_eq!(delete_word("feat rut "), "feat ");
        assert_eq!(delete_word("feat"), "");
        assert_eq!(delete_word(""), "");
    }

    #[test]
    fn counts_are_zero_without_a_snapshot() {
        let state = AppState::new("/tmp/fleet", Instant::now());
        assert_eq!(counts(&state), (0, 0));
        assert!(visible_worktrees(&state).is_empty());
    }

    #[test]
    fn the_retained_chip_appears_only_after_the_input_is_left() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.filter = FilterState {
            query: "rut".to_owned(),
            editing: true,
        };
        assert!(retained_chip(&state).is_none(), "still typing");
        state.filter.editing = false;
        assert!(
            retained_chip(&state).is_some(),
            "filter is hidden otherwise"
        );
        state.filter.query.clear();
        assert!(retained_chip(&state).is_none());
    }

    #[test]
    fn escape_is_two_staged_and_never_quits() {
        assert_eq!(filter_escape(true), FilterEscape::LeaveInput);
        assert_eq!(filter_escape(false), FilterEscape::ClearFilter);
    }
}
