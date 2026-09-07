//! §3.10 Filter bar (`/`) — *narrow this list without moving it*.

use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::filter as filter_actions,
    dialogs::typed_char,
    presentation::{filter_counts, filter_target},
    screens::hub::HubCtx,
    state::{AppState, HubPane, HubTab, Screen},
};

#[cfg(test)]
use super::host::{SessionTransport, open_worktree};
#[cfg(test)]
use crate::{
    presentation::{DisplayedPr, DisplayedTarget},
    state::RepoScope,
};
#[cfg(test)]
use fleet_proto::{request::RequestBody, response::ResponseBody};

/// How many rows the focused list shows, and how many it has in total (`2/12`).
#[must_use]
fn counts(state: &AppState) -> (usize, usize) {
    filter_counts(state)
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
pub(crate) fn render(
    state: &Entity<AppState>,
    hub: HubCtx,
    focus: &FocusHandle,
    _window: &mut Window,
    _cx: &mut App,
) -> AnyElement {
    let accept_state = state.clone();
    let accept_hub = hub;
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
                    reset_cursor(app);
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
            accept(&accept_state, &accept_hub, cx);
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
        match (app.hub_pane, &app.screen) {
            (HubPane::Repos, _) => {
                app.cursors.repos = crate::state::move_cursor(app.cursors.repos, delta, shown)
            }
            (
                HubPane::List,
                Screen::Hub {
                    tab: HubTab::Worktrees,
                },
            ) => {
                app.cursors.worktrees =
                    crate::state::move_cursor(app.cursors.worktrees, delta, shown)
            }
            (HubPane::List, Screen::Hub { tab: HubTab::Prs }) => match app.pr_tab {
                fleet_core::github::PrTab::Mine => {
                    app.cursors.prs_mine =
                        crate::state::move_cursor(app.cursors.prs_mine, delta, shown)
                }
                fleet_core::github::PrTab::Review => {
                    app.cursors.prs_review =
                        crate::state::move_cursor(app.cursors.prs_review, delta, shown)
                }
            },
            (HubPane::List, Screen::Workspace { .. }) => {}
        }
        cx.notify();
    });
}

fn reset_cursor(app: &mut AppState) {
    match (app.hub_pane, &app.screen) {
        (HubPane::Repos, _) => app.cursors.repos = 0,
        (
            HubPane::List,
            Screen::Hub {
                tab: HubTab::Worktrees,
            },
        ) => app.cursors.worktrees = 0,
        (HubPane::List, Screen::Hub { tab: HubTab::Prs }) => match app.pr_tab {
            fleet_core::github::PrTab::Mine => app.cursors.prs_mine = 0,
            fleet_core::github::PrTab::Review => app.cursors.prs_review = 0,
        },
        (HubPane::List, Screen::Workspace { .. }) => {}
    }
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
fn accept_for_test<T: SessionTransport>(state: &Entity<AppState>, bridge: &T, cx: &mut App) {
    let Some(target) = filter_target(state.read(cx)) else {
        return;
    };
    state.update(cx, |app, cx| {
        app.close_overlay();
        app.filter = crate::state::FilterState::default();
        cx.notify();
    });
    match target {
        DisplayedTarget::AllRepos => select_repo(state, RepoScope::All, cx),
        DisplayedTarget::Repo(repo) => select_repo(state, RepoScope::Repo(repo), cx),
        DisplayedTarget::CloneFailed { job, .. } => {
            state.update(cx, |app, cx| {
                app.open_overlay(crate::state::Overlay::Jobs);
                app.jobs_focus = job;
                cx.notify();
            });
        }
        DisplayedTarget::Worktree(id) => open_worktree(id, state, bridge, cx),
        DisplayedTarget::PullRequest(_) => {}
    }
}

#[cfg(test)]
fn select_repo(state: &Entity<AppState>, scope: RepoScope, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.scope = scope;
        app.cursors.worktrees = 0;
        app.hub_pane = HubPane::List;
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Instant};

    use super::*;

    type TestReplySender =
        async_channel::Sender<Result<ResponseBody, fleet_proto::error::ProtoError>>;

    #[derive(Clone, Default)]
    struct FakeTransport {
        requests: Rc<RefCell<Vec<RequestBody>>>,
        replies: Rc<RefCell<VecDeque<TestReplySender>>>,
    }

    impl SessionTransport for FakeTransport {
        fn send(&self, body: RequestBody) {
            self.requests.borrow_mut().push(body);
        }

        fn request(
            &self,
            body: RequestBody,
        ) -> async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>> {
            let (sender, receiver) = async_channel::bounded(1);
            self.requests.borrow_mut().push(body);
            self.replies.borrow_mut().push_back(sender);
            receiver
        }
    }

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
            state.displayed_hub.repos = vec![displayed_repo(
                crate::presentation::DisplayedRepoKind::CloneFailed,
                Some("acme/api"),
                Some("clone-job"),
            )];
            state
        });
        let transport = FakeTransport::default();

        cx.update(|cx| accept_for_test(&state, &transport, cx));

        cx.read(|cx| {
            let app = state.read(cx);
            assert!(matches!(app.overlay, Some(crate::state::Overlay::Jobs)));
            assert_eq!(
                app.jobs_focus.as_ref().map(|job| job.as_str()),
                Some("clone-job")
            );
            assert_eq!(app.cursors.worktrees, 0);
        });
    }

    #[gpui::test]
    fn filter_repo_activation_resets_worktree_cursor(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet", Instant::now());
            state.hub_pane = HubPane::Repos;
            state.cursors.worktrees = 7;
            state.displayed_hub.repos = vec![displayed_repo(
                crate::presentation::DisplayedRepoKind::Repo,
                Some("acme/api"),
                None,
            )];
            state
        });
        let transport = FakeTransport::default();

        cx.update(|cx| accept_for_test(&state, &transport, cx));

        cx.read(|cx| {
            let app = state.read(cx);
            assert_eq!(
                app.scope,
                RepoScope::Repo("acme/api".parse().expect("repo id"))
            );
            assert_eq!(app.cursors.worktrees, 0);
            assert_eq!(app.hub_pane, HubPane::List);
        });
    }
}
