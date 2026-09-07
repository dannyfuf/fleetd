use super::Shell;
use crate::{
    actions::{fleet, quit_daemon_dialog, quit_dialog},
    dialogs::Dialogs,
    shell::quit::{QuitDecision, StopDecision, quit_decision, running_count, stop_decision},
    state::{DaemonLink, Overlay},
};
use fleet_proto::{error::ProtoError, request::RequestBody, response::ResponseBody};
use gpui::{Context, Window};

#[derive(Debug, PartialEq, Eq)]
enum QuitRequestOutcome {
    Quit,
    KeepOpen(String),
}

fn never_warn_outcome(answer: Option<Result<ResponseBody, ProtoError>>) -> QuitRequestOutcome {
    match answer {
        Some(Ok(ResponseBody::Config(_))) => QuitRequestOutcome::Quit,
        Some(Ok(_)) => QuitRequestOutcome::KeepOpen(
            "quit preference was not saved: daemon returned an unexpected response".to_owned(),
        ),
        Some(Err(error)) => QuitRequestOutcome::KeepOpen(format!(
            "quit preference was not saved: {}",
            error.message
        )),
        None => QuitRequestOutcome::KeepOpen(
            "quit preference was not saved: the daemon did not answer".to_owned(),
        ),
    }
}

fn shutdown_outcome(answer: Option<Result<ResponseBody, ProtoError>>) -> QuitRequestOutcome {
    match answer {
        Some(Ok(ResponseBody::ShuttingDown)) => QuitRequestOutcome::Quit,
        Some(Ok(_)) => QuitRequestOutcome::KeepOpen(
            "fleetd was not stopped: daemon returned an unexpected response".to_owned(),
        ),
        Some(Err(error)) => {
            QuitRequestOutcome::KeepOpen(format!("fleetd was not stopped: {}", error.message))
        }
        None => QuitRequestOutcome::KeepOpen(
            "fleetd was not stopped: the daemon did not answer".to_owned(),
        ),
    }
}

fn retain_after_quit_refusal(state: &mut crate::state::AppState, message: String) {
    state.close_overlay();
    super::record_request_failure(state, message);
}

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
        let reply = self.bridge.request(RequestBody::SetConfig {
            patch: serde_json::json!({ "jobs": { "warnBeforeQuit": false } }),
        });
        cx.spawn(async move |shell, cx| {
            let answer = reply.recv().await.ok();
            let _ = shell.update(cx, |shell, cx| match never_warn_outcome(answer) {
                QuitRequestOutcome::Quit => {
                    shell.state.update(cx, |state, _| {
                        state.warn_before_quit = false;
                    });
                    shell.quit_now(cx);
                }
                QuitRequestOutcome::KeepOpen(message) => {
                    shell.state.update(cx, |state, cx| {
                        retain_after_quit_refusal(state, message);
                        cx.notify();
                    });
                }
            });
        })
        .detach();
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
        cx.spawn(async move |shell, cx| {
            let answer = reply.recv().await.ok();
            let _ = shell.update(cx, |shell, cx| match shutdown_outcome(answer) {
                QuitRequestOutcome::Quit => shell.quit_now(cx),
                QuitRequestOutcome::KeepOpen(message) => {
                    shell.state.update(cx, |state, cx| {
                        retain_after_quit_refusal(state, message);
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{BridgeEvent, EffectiveConfig};
    use fleet_proto::error::ErrorKind;
    use std::time::Instant;

    fn refusal(message: &str) -> Option<Result<ResponseBody, ProtoError>> {
        Some(Err(ProtoError {
            kind: ErrorKind::Conflict,
            message: message.to_owned(),
        }))
    }

    #[test]
    fn never_warn_waits_for_config_ack() {
        assert!(matches!(
            never_warn_outcome(None),
            QuitRequestOutcome::KeepOpen(_)
        ));
        assert!(matches!(
            never_warn_outcome(refusal("read-only config")),
            QuitRequestOutcome::KeepOpen(message) if message.contains("read-only config")
        ));
        let config = fleet_core::config::default_config("/tmp/fleet");
        assert_eq!(
            never_warn_outcome(Some(Ok(ResponseBody::Config(config)))),
            QuitRequestOutcome::Quit
        );

        let mut state = crate::state::AppState::new("/tmp/fleet", Instant::now());
        state.open_overlay(Overlay::Dialog(Dialogs::Quit));
        retain_after_quit_refusal(&mut state, "read-only config".to_owned());
        assert!(state.overlay.is_none());
        assert_eq!(
            state.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("read-only config")
        );
    }

    #[test]
    fn shutdown_refusal_keeps_window_open() {
        assert!(matches!(
            shutdown_outcome(refusal("sessions refused to stop")),
            QuitRequestOutcome::KeepOpen(message) if message.contains("sessions refused to stop")
        ));
        assert_eq!(
            shutdown_outcome(Some(Ok(ResponseBody::ShuttingDown))),
            QuitRequestOutcome::Quit
        );

        let mut state = crate::state::AppState::new("/tmp/fleet", Instant::now());
        state.open_overlay(Overlay::Dialog(Dialogs::QuitDaemon));
        retain_after_quit_refusal(&mut state, "sessions refused to stop".to_owned());
        assert!(state.overlay.is_none(), "the dialog scrim must be removed");
        assert_eq!(
            state.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("sessions refused to stop")
        );
    }

    #[test]
    fn quit_uses_effective_warn_before_quit() {
        let now = Instant::now();
        let mut state = crate::state::AppState::new("/tmp/fleet", now);
        let mut config = fleet_core::config::default_config("/tmp/fleet");
        config.jobs.warn_before_quit = false;

        state.apply_bridge_event(
            BridgeEvent::EffectiveConfig(EffectiveConfig::from_config(&config)),
            now,
        );

        assert!(!state.warn_before_quit);
        assert_eq!(
            quit_decision(state.warn_before_quit, 3),
            QuitDecision::QuitNow
        );
    }
}
