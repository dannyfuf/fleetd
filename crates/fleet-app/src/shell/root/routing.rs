use super::Shell;
use crate::{
    actions::{hub, prefix, scroll, workspace},
    state::{HubPane, HubTab, Screen, TerminalMode},
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
            state.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            state.hub_pane = HubPane::List;
            cx.notify();
        });
    }

    pub(super) fn go_prs(&mut self, _: &hub::GoPrs, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.screen = Screen::Hub { tab: HubTab::Prs };
            state.hub_pane = HubPane::List;
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
            if let Screen::Hub { tab } = &state.screen {
                let next = match tab {
                    HubTab::Worktrees => HubTab::Prs,
                    HubTab::Prs => HubTab::Worktrees,
                };
                state.screen = Screen::Hub { tab: next };
                // §3.10: a filter does not survive a screen change.
                state.filter = crate::state::FilterState::default();
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
            state.leave_prefix();
            state.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            state.hub_pane = HubPane::List;
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
