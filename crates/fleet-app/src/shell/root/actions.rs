use super::{
    Shell, first_run_import_allowed,
    focus::{FocusOwnerKeys, STALE_KEY_CAPACITY},
};
use crate::{
    actions::{
        confirm, dialog, filter, first_run as first_run_actions, fleet, help, hub, jobs, palette,
        repos,
    },
    dialogs::Dialogs,
    state::Overlay,
};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::Icon;
use gpui::{Context, Div, KeyDownEvent, Window, prelude::*};
use std::{cell::RefCell, rc::Rc, time::Instant};

impl Shell {
    pub(super) fn open(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.open_overlay(overlay);
            cx.notify();
        });
    }

    pub(super) fn close_overlay(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.close_overlay() {
                cx.notify();
            }
        });
    }

    pub(super) fn open_palette(
        &mut self,
        _: &fleet::OpenPalette,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Palette, cx);
    }

    pub(super) fn open_settings(
        &mut self,
        _: &fleet::OpenSettings,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Dialog(Dialogs::Settings), cx);
    }

    pub(super) fn open_help(
        &mut self,
        _: &fleet::OpenHelp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Dialog(Dialogs::Help), cx);
    }

    pub(super) fn open_jobs(
        &mut self,
        _: &fleet::OpenJobs,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Jobs, cx);
    }

    pub(super) fn go_jobs(&mut self, _: &hub::GoJobs, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Jobs, cx);
    }

    /// `!` focuses the sticky error, which means opening the Jobs panel on the failed job.
    pub(super) fn focus_sticky_error(
        &mut self,
        _: &fleet::FocusStickyError,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Jobs, cx);
    }

    pub(super) fn open_filter(
        &mut self,
        _: &hub::OpenFilter,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.filter.editing = true;
            state.open_overlay(Overlay::Filter);
            cx.notify();
        });
    }

    pub(super) fn cancel(&mut self, _: &fleet::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.cancel() {
                cx.notify();
            }
        });
    }

    pub(super) fn filter_escape(
        &mut self,
        _: &filter::Escape,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            if state.cancel() {
                cx.notify();
            }
        });
    }

    pub(super) fn close_palette(
        &mut self,
        _: &palette::Close,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(cx);
    }

    pub(super) fn close_jobs(&mut self, _: &jobs::Close, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    pub(super) fn close_help(&mut self, _: &help::Close, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    pub(super) fn cancel_dialog(
        &mut self,
        _: &dialog::Cancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(cx);
    }

    pub(super) fn reject_confirm(
        &mut self,
        _: &confirm::Reject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(cx);
    }

    /// The first-run card has no Hub listeners, so its advertised actions live on the shell.
    pub(super) fn first_run_import(
        &mut self,
        _: &first_run_actions::Import,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = self.state.read(cx).home.clone();
        let task = cx
            .background_executor()
            .spawn(async move { home.join("state.json").exists() });
        cx.spawn(async move |shell, cx| {
            let exists = task.await;
            let _ = shell.update(cx, |shell, cx| {
                let allowed = first_run_import_allowed(shell.state.read(cx).is_first_run(), exists);
                if allowed {
                    shell.bridge.send(RequestBody::ImportFromSwarm);
                } else {
                    shell.state.update(cx, |state, cx| {
                        state.toast_short(
                            "Fleet state already exists; import was not started",
                            Icon::Info,
                            Instant::now(),
                        );
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    /// `N` on the first-run card: create the first context.
    pub(super) fn new_context(
        &mut self,
        _: &hub::NewContext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Dialog(Dialogs::NewContext), cx);
    }

    /// `n` on the first-run card: clone the first repository.
    pub(super) fn clone_repo(&mut self, _: &repos::Clone, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Dialog(Dialogs::CloneRepo), cx);
    }

    /// Both normal and daemon surfaces retain every global action.
    pub(super) fn with_actions(
        root: Div,
        focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
        cx: &mut Context<Self>,
    ) -> Div {
        root.capture_key_down(move |event: &KeyDownEvent, _window, cx| {
            let mut keys = focus_owner_keys.borrow_mut();
            if !keys.awaiting_capture {
                return;
            }
            if let Some(dropped) = keys.capture(event.clone()) {
                tracing::warn!(
                    keystroke = %dropped.keystroke,
                    capacity = STALE_KEY_CAPACITY,
                    "dropping oldest stale-frame key event"
                );
            }
            cx.stop_propagation();
        })
        // Global
        .on_action(cx.listener(Self::quit))
        .on_action(cx.listener(Self::quit_and_stop_daemon))
        .on_action(cx.listener(Self::open_palette))
        .on_action(cx.listener(Self::open_settings))
        .on_action(cx.listener(Self::open_help))
        .on_action(cx.listener(Self::open_jobs))
        .on_action(cx.listener(Self::open_agent_claude))
        .on_action(cx.listener(Self::open_agent_opencode))
        .on_action(cx.listener(Self::focus_sticky_error))
        .on_action(cx.listener(Self::cancel))
        // First run (§3.13) — fallbacks for the three keys the card advertises
        .on_action(cx.listener(Self::first_run_import))
        .on_action(cx.listener(Self::new_context))
        .on_action(cx.listener(Self::clone_repo))
        // Hub navigation
        .on_action(cx.listener(Self::focus_prev_pane))
        .on_action(cx.listener(Self::focus_next_pane))
        .on_action(cx.listener(Self::go_repos))
        .on_action(cx.listener(Self::go_worktrees))
        .on_action(cx.listener(Self::go_prs))
        .on_action(cx.listener(Self::go_jobs))
        .on_action(cx.listener(Self::toggle_pr_screen))
        .on_action(cx.listener(Self::open_filter))
        .on_action(cx.listener(Self::toggle_detail))
        .on_action(cx.listener(Self::toggle_rail))
        // Workspace modes
        .on_action(cx.listener(Self::enter_prefix))
        .on_action(cx.listener(Self::cancel_prefix))
        .on_action(cx.listener(Self::prefix_go_hub))
        .on_action(cx.listener(Self::prefix_enter_scroll))
        .on_action(cx.listener(Self::prefix_toggle_zoom))
        .on_action(cx.listener(Self::leave_scroll))
        .on_action(cx.listener(Self::escape_scroll))
        // Overlays
        .on_action(cx.listener(Self::close_palette))
        .on_action(cx.listener(Self::close_jobs))
        .on_action(cx.listener(Self::close_help))
        .on_action(cx.listener(Self::cancel_dialog))
        .on_action(cx.listener(Self::reject_confirm))
        .on_action(cx.listener(Self::filter_escape))
        // Daemon
        .on_action(cx.listener(Self::daemon_retry))
        .on_action(cx.listener(Self::daemon_reconnect))
        .on_action(cx.listener(Self::daemon_open_log))
        .on_action(cx.listener(Self::daemon_dismiss_banner))
        .on_action(cx.listener(Self::run_doctor))
        // Quit dialogs
        .on_action(cx.listener(Self::accept_quit))
        .on_action(cx.listener(Self::reject_quit))
        .on_action(cx.listener(Self::quit_dialog_jobs))
        .on_action(cx.listener(Self::never_warn))
        .on_action(cx.listener(Self::accept_stop_daemon))
        .on_action(cx.listener(Self::reject_stop_daemon))
    }
}
