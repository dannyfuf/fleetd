use super::Shell;
use crate::{
    actions::{fleet, quit_daemon_dialog, quit_dialog},
    dialogs::Dialogs,
    shell::quit::{QuitDecision, StopDecision, quit_decision, running_count, stop_decision},
    state::{DaemonLink, Overlay},
};
use fleet_proto::request::RequestBody;
use gpui::{Context, Window};

impl Shell {
    pub(super) fn quit(&mut self, _: &fleet::Quit, _: &mut Window, cx: &mut Context<Self>) {
        let (warn, running) = {
            let state = self.state.read(cx);
            // §3.8.8 exists to say "these keep running **in fleetd**". While the daemon is
            // starting or will not start, the last snapshot's running jobs belong to a daemon
            // that is already gone, so there is nothing for the confirm to promise and
            // `ctrl-q` quits — which is exactly what KEYMAP guarantees on those surfaces.
            let daemon_gone = matches!(
                state.daemon,
                DaemonLink::Starting | DaemonLink::Failed { .. }
            );
            (
                state.warn_before_quit,
                if daemon_gone {
                    0
                } else {
                    state
                        .snapshot
                        .as_ref()
                        .map_or(0, |snapshot| running_count(&snapshot.jobs))
                },
            )
        };
        match quit_decision(warn, running) {
            QuitDecision::QuitNow => self.quit_now(cx),
            QuitDecision::Confirm => self.open(Overlay::Dialog(Dialogs::Quit), cx),
        }
    }

    pub(super) fn quit_and_stop_daemon(
        &mut self,
        _: &fleet::QuitAndStopDaemon,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (running, sessions) = {
            let state = self.state.read(cx);
            state.snapshot.as_ref().map_or((0, 0), |snapshot| {
                (running_count(&snapshot.jobs), snapshot.sessions.len())
            })
        };
        match stop_decision(running, sessions) {
            StopDecision::StopNow => self.stop_daemon_and_quit(cx),
            StopDecision::Confirm => self.open(Overlay::Dialog(Dialogs::QuitDaemon), cx),
        }
    }

    pub(super) fn accept_quit(
        &mut self,
        _: &quit_dialog::Accept,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.quit_now(cx);
    }

    pub(super) fn reject_quit(
        &mut self,
        _: &quit_dialog::Reject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(cx);
    }

    pub(super) fn quit_dialog_jobs(
        &mut self,
        _: &quit_dialog::OpenJobs,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let preserve = self
            .state
            .read(cx)
            .active_session()
            .and_then(|session| session.active_terminal);
        self.agent_popup.detach(&self.bridge, preserve);
        self.state.update(cx, |state, cx| {
            state.hide_agent_popup();
            state.open_overlay(Overlay::Jobs);
            cx.notify();
        });
    }

    /// `W` in §3.8.8: a legitimate "don't ask again", because nothing is lost by quitting.
    pub(super) fn never_warn(
        &mut self,
        _: &quit_dialog::NeverWarn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bridge.send(RequestBody::SetConfig {
            patch: serde_json::json!({ "jobs": { "warnBeforeQuit": false } }),
        });
        self.state
            .update(cx, |state, _| state.warn_before_quit = false);
        self.quit_now(cx);
    }

    pub(super) fn accept_stop_daemon(
        &mut self,
        _: &quit_daemon_dialog::Accept,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_daemon_and_quit(cx);
    }

    pub(super) fn reject_stop_daemon(
        &mut self,
        _: &quit_daemon_dialog::Reject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(cx);
    }

    fn quit_now(&mut self, cx: &mut Context<Self>) {
        self.bridge.shutdown();
        cx.quit();
    }

    fn stop_daemon_and_quit(&mut self, cx: &mut Context<Self>) {
        let reply = self.bridge.request(RequestBody::DaemonShutdown {
            stop_sessions: true,
        });
        let bridge = self.bridge.clone();
        cx.spawn(async move |_, cx| {
            let _ignored = reply.recv().await;
            bridge.shutdown();
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}
