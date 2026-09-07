use super::Shell;
use crate::{
    actions::{hub, prefix, scroll, workspace},
    state::{AppState, FilterState, HubPane, HubTab, Screen, TerminalMode},
};
use fleet_ui_kit::Icon;
use gpui::{Context, Window};
use std::time::Instant;

impl Shell {
    pub(super) fn focus_prev_pane(
        &mut self,
        _: &hub::FocusPrevPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_pane(HubPane::Repos, cx);
    }

    pub(super) fn focus_next_pane(
        &mut self,
        _: &hub::FocusNextPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_pane(HubPane::List, cx);
    }

    pub(super) fn go_repos(&mut self, _: &hub::GoRepos, _: &mut Window, cx: &mut Context<Self>) {
        self.set_pane(HubPane::Repos, cx);
    }

    pub(super) fn go_worktrees(
        &mut self,
        _: &hub::GoWorktrees,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            go_worktrees_state(state);
            cx.notify();
        });
    }

    pub(super) fn go_prs(&mut self, _: &hub::GoPrs, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            go_prs_state(state);
            cx.notify();
        });
    }

    pub(super) fn toggle_pr_screen(
        &mut self,
        _: &hub::TogglePrScreen,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            if toggle_pr_screen_state(state) {
                cx.notify();
            }
        });
    }

    fn set_pane(&mut self, pane: HubPane, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if matches!(state.screen, Screen::Hub { .. }) && state.hub_pane != pane {
                state.hub_pane = pane;
                cx.notify();
            }
        });
    }

    pub(super) fn toggle_detail(
        &mut self,
        _: &hub::ToggleDetail,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.detail_open = !state.detail_open;
            cx.notify();
        });
    }

    pub(super) fn toggle_rail(
        &mut self,
        _: &hub::ToggleRepoRail,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.rail_collapsed = !state.rail_collapsed;
            cx.notify();
        });
    }

    pub(super) fn enter_prefix(
        &mut self,
        _: &workspace::EnterPrefix,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.enter_prefix();
            cx.notify();
        });
    }

    pub(super) fn cancel_prefix(
        &mut self,
        _: &prefix::Cancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.leave_prefix();
            cx.notify();
        });
    }

    pub(super) fn prefix_go_hub(
        &mut self,
        _: &prefix::GoHub,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            prefix_go_hub_state(state);
            cx.notify();
        });
    }

    pub(super) fn prefix_enter_scroll(
        &mut self,
        _: &prefix::EnterScroll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            let alt_screen = state
                .active_grid()
                .is_some_and(|grid| grid.modes.alt_screen);
            // A Fleet-drawn tab has no scrollback to enter: there is no PTY behind it and the
            // pane scrolls itself. Saying so is better than a Scroll mode that answers nothing.
            let native = state.active_terminal_is_native();
            state.leave_prefix();
            if native {
                state.toast_short(
                    "no scrollback in this tab",
                    Icon::ChevronsUp,
                    Instant::now(),
                );
            } else if alt_screen {
                // §3.6: scroll mode is suppressed while an alt-screen app is running.
                state.toast_short(
                    "no scrollback in alt-screen",
                    Icon::ChevronsUp,
                    Instant::now(),
                );
            } else {
                state.terminal_mode = TerminalMode::Scroll;
            }
            cx.notify();
        });
    }

    pub(super) fn prefix_toggle_zoom(
        &mut self,
        _: &prefix::ToggleZoom,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.leave_prefix();
            state.zoomed = !state.zoomed;
            cx.notify();
        });
    }

    pub(super) fn leave_scroll(
        &mut self,
        _: &scroll::Exit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.exit_scroll(cx);
    }

    pub(super) fn escape_scroll(
        &mut self,
        _: &scroll::Escape,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.exit_scroll(cx);
    }

    fn exit_scroll(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.terminal_mode == TerminalMode::Scroll {
                state.terminal_mode = state.resting_terminal_mode();
                cx.notify();
            }
        });
    }
}

fn go_worktrees_state(state: &mut AppState) {
    route_to_hub(state, HubTab::Worktrees);
}

fn go_prs_state(state: &mut AppState) {
    route_to_hub(state, HubTab::Prs);
}

fn toggle_pr_screen_state(state: &mut AppState) -> bool {
    let Screen::Hub { tab } = &state.screen else {
        return false;
    };
    let next = match tab {
        HubTab::Worktrees => HubTab::Prs,
        HubTab::Prs | HubTab::Board => HubTab::Worktrees,
    };
    route_to_hub(state, next);
    true
}

fn prefix_go_hub_state(state: &mut AppState) {
    state.leave_prefix();
    route_to_hub(state, HubTab::Worktrees);
}

/// Routes every equivalent Hub action through the same §3.10 filter policy.
fn route_to_hub(state: &mut AppState, tab: HubTab) {
    let destination = Screen::Hub { tab };
    if state.screen != destination {
        state.filter = FilterState::default();
    }
    state.screen = destination;
    state.hub_pane = HubPane::List;
}

#[cfg(test)]
mod tests {
    use super::*;

    type RouteCase = (&'static str, fn(&mut AppState), HubTab, HubTab);

    #[test]
    fn equivalent_hub_routes_apply_identical_filter_policy() {
        let routes: [RouteCase; 4] = [
            (
                "go_worktrees",
                go_worktrees_state,
                HubTab::Prs,
                HubTab::Worktrees,
            ),
            ("go_prs", go_prs_state, HubTab::Worktrees, HubTab::Prs),
            (
                "toggle_pr_screen",
                |state| {
                    assert!(toggle_pr_screen_state(state));
                },
                HubTab::Worktrees,
                HubTab::Prs,
            ),
            (
                "prefix_go_hub",
                prefix_go_hub_state,
                HubTab::Prs,
                HubTab::Worktrees,
            ),
        ];

        for (name, route, from, to) in routes {
            let mut state = AppState::new("/tmp/fleet", Instant::now());
            state.screen = Screen::Hub { tab: from };
            state.filter.query = "payroll".to_owned();
            route(&mut state);
            assert_eq!(state.screen, Screen::Hub { tab: to }, "{name}");
            assert_eq!(state.filter, FilterState::default(), "{name}");
        }

        for (name, route, tab) in [
            (
                "go_worktrees",
                go_worktrees_state as fn(&mut AppState),
                HubTab::Worktrees,
            ),
            ("go_prs", go_prs_state as fn(&mut AppState), HubTab::Prs),
            (
                "prefix_go_hub",
                prefix_go_hub_state as fn(&mut AppState),
                HubTab::Worktrees,
            ),
        ] {
            let mut state = AppState::new("/tmp/fleet", Instant::now());
            state.screen = Screen::Hub { tab };
            state.filter.query = "review".to_owned();
            route(&mut state);
            assert_eq!(state.filter.query, "review", "{name}");
        }

        let mut outside_hub = AppState::new("/tmp/fleet", Instant::now());
        outside_hub.screen = Screen::Workspace {
            session: "owner/repo"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        outside_hub.filter.query = "review".to_owned();
        assert!(!toggle_pr_screen_state(&mut outside_hub));
        assert_eq!(outside_hub.filter.query, "review");
    }
}
