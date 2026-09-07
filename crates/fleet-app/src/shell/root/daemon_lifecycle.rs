use super::Shell;
use crate::{actions::daemon as daemon_actions, state::DaemonLink};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use gpui::{Context, Window};
use std::time::Instant;

impl Shell {
    pub(super) fn daemon_retry(
        &mut self,
        _: &daemon_actions::Retry,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.retry_daemon(cx);
    }

    pub(super) fn daemon_reconnect(
        &mut self,
        _: &daemon_actions::Reconnect,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.retry_daemon(cx);
    }

    fn retry_daemon(&mut self, cx: &mut Context<Self>) {
        self.bridge.reconnect();
        self.state.update(cx, |state, cx| {
            state.daemon = DaemonLink::Starting;
            state.daemon_since = Instant::now();
            cx.notify();
        });
    }

    pub(super) fn daemon_open_log(
        &mut self,
        _: &daemon_actions::OpenLog,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.state.read(cx).daemon_log_path();
        cx.open_with_system(&path);
    }

    pub(super) fn daemon_dismiss_banner(
        &mut self,
        _: &daemon_actions::DismissBanner,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let showed_doctor = self.state.update(cx, |state, cx| {
            let showed = state.doctor.take().is_some();
            if showed {
                cx.notify();
            }
            showed
        });
        if showed_doctor {
            return;
        }
        self.state.update(cx, |state, cx| {
            if let DaemonLink::Lost { attempt, .. } = state.daemon {
                state.daemon = DaemonLink::Lost {
                    attempt,
                    dismissed: true,
                };
                cx.notify();
            }
        });
    }

    pub(super) fn run_doctor(
        &mut self,
        _: &daemon_actions::RunDoctor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reply = self.bridge.request(RequestBody::Doctor);
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let answer = reply.recv().await;
            cx.update(|cx| {
                state.update(cx, |app, cx| {
                    match answer {
                        Ok(Ok(ResponseBody::Doctor(checks))) => app.doctor = Some(checks),
                        // §3.12 B is exactly where `D` matters and exactly where the bridge is
                        // offline, so the refusal has to land in the sticky slot (§1.8) rather
                        // than be dropped on the floor.
                        Ok(Err(error)) => {
                            app.sticky_error = Some(crate::state::StickyError {
                                text: error.message,
                                job: None,
                                retryable: false,
                            });
                        }
                        Ok(Ok(_)) | Err(_) => {
                            app.sticky_error = Some(crate::state::StickyError {
                                text: "doctor: the daemon did not answer".to_owned(),
                                job: None,
                                retryable: false,
                            });
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
