use super::*;
use crate::state::{AppState, DaemonLink};
use fleet_core::paths::FleetHome;
use gpui::{Context, Entity, Render, ScrollHandle, Subscription, Window};

#[derive(Default)]
enum Content {
    #[default]
    Empty,
    Report {
        checks: Vec<DoctorCheck>,
        protocol: Option<u32>,
        rows: Vec<DoctorRow>,
        error: Option<SharedString>,
    },
    Failure {
        message: SharedString,
        stale_socket: bool,
        kind: DaemonFailure,
        detail: SharedString,
        lines: Vec<SharedString>,
        error: Option<SharedString>,
    },
}

impl Content {
    fn update(&mut self, state: &AppState) -> bool {
        if let Some(checks) = &state.doctor {
            let protocol = state.snapshot.as_ref().map(|_| APP_PROTOCOL);
            if let Self::Report {
                checks: previous,
                protocol: previous_protocol,
                ..
            } = self
                && previous == checks
                && *previous_protocol == protocol
            {
                return false;
            }
            let mut rows = vec![protocol_row(protocol)];
            rows.extend(doctor_rows(checks));
            *self = Self::Report {
                checks: checks.clone(),
                protocol,
                rows,
                error: summary(checks).err().map(Into::into),
            };
            return true;
        }
        if let DaemonLink::Failed {
            message,
            log_tail,
            stale_socket,
            protocol_mismatch,
        } = &state.daemon
        {
            let kind = DaemonFailure::classify(*protocol_mismatch, *stale_socket);
            let error = state
                .sticky_error
                .as_ref()
                .filter(|error| error.text.starts_with("doctor could not run:"))
                .map(|error| SharedString::from(error.text.clone()));
            if let Self::Failure {
                message: previous,
                stale_socket: previous_stale,
                kind: previous_kind,
                lines,
                error: previous_error,
                ..
            } = self
                && previous.as_str() == message
                && previous_stale == stale_socket
                && *previous_kind == kind
                && *previous_error == error
                && lines.len() == log_tail.len()
                && lines
                    .iter()
                    .zip(log_tail)
                    .all(|(left, right)| left.as_str() == right)
            {
                return false;
            }
            let socket: SharedString = FleetHome::new(&state.home)
                .socket_path()
                .display()
                .to_string()
                .into();
            *self = Self::Failure {
                message: SharedString::new(message),
                stale_socket: *stale_socket,
                detail: failure_detail(kind, message, &socket).into(),
                kind,
                lines: log_tail
                    .iter()
                    .map(|line| SharedString::new(line.as_str()))
                    .collect(),
                error,
            };
            return true;
        }
        let changed = !matches!(self, Self::Empty);
        *self = Self::Empty;
        changed
    }
}

/// Rows, summaries, and scrolling belong to the diagnostic surface. Changes to theme or its
/// actual report/failure inputs invalidate the definite-size cached child view.
pub(crate) struct DiagnosticView {
    content: Content,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl DiagnosticView {
    pub(crate) fn new(state: &Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(state, |view, state, cx| {
                if view.content.update(state.read(cx)) {
                    cx.notify();
                }
            }),
            cx.observe_global::<fleet_ui_kit::Theme>(|_, cx| cx.notify()),
        ];
        let mut content = Content::default();
        content.update(state.read(cx));
        Self {
            content,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }
}

impl Render for DiagnosticView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.content {
            Content::Empty => div().into_any_element(),
            Content::Report { rows, error, .. } => {
                report_view(rows, error.as_ref(), &self.scroll, cx)
            }
            Content::Failure {
                kind,
                detail,
                lines,
                error,
                ..
            } => failure_view(*kind, detail, lines, error.as_ref(), &self.scroll, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_state_updates_reuse_prepared_diagnostics() {
        let mut state = AppState::new("/tmp/fleet", std::time::Instant::now());
        state.doctor = Some(vec![DoctorCheck {
            check: "git".into(),
            status: WireDoctorStatus::Ok,
            detail: "available".into(),
        }]);
        let mut content = Content::default();
        assert!(content.update(&state));
        assert!(!content.update(&state));
        state.breadcrumb_row = Some("other".into());
        assert!(!content.update(&state));
        state.doctor.as_mut().unwrap()[0].detail = "updated".into();
        assert!(content.update(&state));
        state.doctor = None;
        assert!(content.update(&state));
    }

    #[test]
    fn failure_inputs_invalidate_without_recopying_an_unchanged_tail() {
        let mut state = AppState::new("/tmp/fleet", std::time::Instant::now());
        state.daemon = DaemonLink::Failed {
            message: "could not start".into(),
            log_tail: vec!["exited".into()],
            stale_socket: false,
            protocol_mismatch: false,
        };
        let mut content = Content::default();
        assert!(content.update(&state));
        assert!(!content.update(&state));
        if let DaemonLink::Failed { log_tail, .. } = &mut state.daemon {
            log_tail.push("new detail".into());
        }
        assert!(content.update(&state));
        if let DaemonLink::Failed { stale_socket, .. } = &mut state.daemon {
            *stale_socket = true;
        }
        assert!(content.update(&state));
        let Content::Failure {
            detail,
            lines,
            error,
            ..
        } = &content
        else {
            panic!("failure content");
        };
        assert!(detail.contains("is stale"));
        assert_eq!(lines.len(), 2);
        assert!(error.is_none());
    }

    #[test]
    fn doctor_refusal_is_projected_without_replacing_the_failure_surface() {
        let mut state = AppState::new("/tmp/fleet", std::time::Instant::now());
        state.daemon = DaemonLink::Failed {
            message: "could not start".into(),
            log_tail: vec!["exited".into()],
            stale_socket: false,
            protocol_mismatch: false,
        };
        state.sticky_error = Some(crate::state::StickyError {
            text: "doctor could not run: bridge unavailable".into(),
            job: None,
            retryable: false,
        });

        let mut content = Content::default();
        assert!(content.update(&state));
        let Content::Failure { error, .. } = &content else {
            panic!("failure content");
        };
        assert_eq!(
            error.as_ref().map(SharedString::as_ref),
            Some("doctor could not run: bridge unavailable")
        );
        assert!(state.doctor.is_none());
    }
}
