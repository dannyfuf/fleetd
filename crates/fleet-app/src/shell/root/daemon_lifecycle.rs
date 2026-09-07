use super::Shell;
use crate::{actions::daemon as daemon_actions, state::DaemonLink};
use fleet_proto::{
    error::ProtoError,
    request::RequestBody,
    response::{DoctorCheck, DoctorStatus, ResponseBody},
};
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
            let answer = reply.recv().await.ok();
            cx.update(|cx| {
                state.update(cx, |app, cx| {
                    apply_doctor_answer(app, answer);
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

struct DoctorAnswer {
    checks: Vec<DoctorCheck>,
    request_failed: bool,
}

fn doctor_answer(answer: Option<Result<ResponseBody, ProtoError>>) -> DoctorAnswer {
    match answer {
        Some(Ok(ResponseBody::Doctor(checks))) => DoctorAnswer {
            checks,
            request_failed: false,
        },
        Some(Err(error)) => DoctorAnswer {
            checks: doctor_failure(error.message),
            request_failed: true,
        },
        Some(Ok(_)) => DoctorAnswer {
            checks: doctor_failure("the daemon returned an unexpected response".to_owned()),
            request_failed: true,
        },
        None => DoctorAnswer {
            checks: doctor_failure("the daemon did not answer".to_owned()),
            request_failed: true,
        },
    }
}

fn apply_doctor_answer(
    app: &mut crate::state::AppState,
    answer: Option<Result<ResponseBody, ProtoError>>,
) {
    let answer = doctor_answer(answer);
    if answer.request_failed && matches!(&app.daemon, DaemonLink::Failed { .. }) {
        let detail = answer
            .checks
            .first()
            .map_or("the daemon did not answer", |check| check.detail.as_str());
        app.doctor = None;
        super::record_request_failure(app, format!("doctor could not run: {detail}"));
        return;
    }
    app.doctor = Some(answer.checks);
}

fn doctor_failure(detail: String) -> Vec<DoctorCheck> {
    vec![DoctorCheck {
        check: "doctor".to_owned(),
        status: DoctorStatus::Fail,
        detail,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_proto::error::ErrorKind;

    fn refusal(message: &str) -> Option<Result<ResponseBody, ProtoError>> {
        Some(Err(ProtoError {
            kind: ErrorKind::Unknown,
            message: message.to_owned(),
        }))
    }

    #[test]
    fn doctor_failure_is_visible_on_daemon_splash() {
        for (answer, expected) in [
            (refusal("bridge unavailable"), "bridge unavailable"),
            (
                Some(Ok(ResponseBody::Ack)),
                "daemon returned an unexpected response",
            ),
            (None, "daemon did not answer"),
        ] {
            let mut state = crate::state::AppState::new("/tmp/fleet", Instant::now());
            apply_doctor_answer(&mut state, answer);
            let checks = state.doctor.as_deref().unwrap_or_default();
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0].status, DoctorStatus::Fail);
            assert!(checks[0].detail.contains(expected));
            assert!(state.sticky_error.is_none());
        }
    }

    #[test]
    fn doctor_refusal_preserves_daemon_failure_surface() {
        let mut state = crate::state::AppState::new("/tmp/fleet", Instant::now());
        state.daemon = DaemonLink::Failed {
            message: "fleetd could not start".to_owned(),
            log_tail: vec!["original log line".to_owned()],
            stale_socket: false,
            protocol_mismatch: false,
        };
        state.doctor = Some(doctor_failure("earlier result".to_owned()));
        apply_doctor_answer(&mut state, refusal("bridge unavailable"));

        assert!(state.doctor.is_none(), "the report must not replace case B");
        assert!(matches!(
            &state.daemon,
            DaemonLink::Failed { message, log_tail, .. }
                if message == "fleetd could not start" && log_tail == &["original log line"]
        ));
        assert!(
            state
                .sticky_error
                .as_ref()
                .is_some_and(|error| error.text.contains("bridge unavailable"))
        );
    }
}
