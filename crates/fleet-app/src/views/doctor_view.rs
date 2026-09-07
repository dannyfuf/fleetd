//! Retained daemon diagnostics and failure presentation.

use fleet_proto::response::{DoctorCheck, DoctorStatus as WireDoctorStatus};
use fleet_ui_kit::{
    ActiveTheme, DoctorRow, DoctorStatus, DoctorTable, Icon, IconSize, KeyHintRow, Text, Tone,
    prelude::*,
};
use gpui::{AnyElement, App, SharedString, div};

/// The wire protocol this build of the app speaks. A daemon that answers with anything else is
/// a version mismatch, not a crash.
pub const APP_PROTOCOL: u32 = fleet_proto::PROTOCOL_VERSION;

/// The heading of the doctor surface.
const TITLE: &str = "Doctor";

/// Maps one wire check onto a kit row.
#[must_use]
fn doctor_row(check: &DoctorCheck) -> DoctorRow {
    DoctorRow::new(
        SharedString::new(check.check.as_str()),
        match check.status {
            WireDoctorStatus::Ok => DoctorStatus::Ok,
            WireDoctorStatus::Warn => DoctorStatus::Warn,
            WireDoctorStatus::Fail => DoctorStatus::Fail,
        },
        SharedString::new(check.detail.as_str()),
    )
}

/// The whole table, in the daemon's order — the order is the diagnosis, so it is never sorted.
#[must_use]
pub fn doctor_rows(checks: &[DoctorCheck]) -> Vec<DoctorRow> {
    checks.iter().map(doctor_row).collect()
}

/// The one-line summary of a doctor run: the first failure, or the all-clear.
///
/// The all-clear is a 1.6 s toast (§2.7 "instant-action acknowledgement"); a failure is sticky.
fn summary(checks: &[DoctorCheck]) -> Result<String, String> {
    match checks
        .iter()
        .find(|check| check.status == WireDoctorStatus::Fail)
    {
        Some(first) => Err(format!("{}: {}", first.check, first.detail)),
        None => Ok(format!("doctor: {} checks ok", checks.len())),
    }
}

/// Why the daemon is not usable, when the client could not establish a link (§3.12 B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonFailure {
    /// A stale `fleetd.sock` is the known cause; retrying after removing it works.
    StaleSocket,
    /// The daemon answered but speaks another protocol. **Retrying cannot fix this.**
    VersionMismatch,
    /// Anything else: the process did not come up.
    WontStart,
}

impl DaemonFailure {
    /// Classifies the bridge's typed failure cause without interpreting human prose.
    #[must_use]
    fn classify(protocol_mismatch: bool, stale_socket: bool) -> Self {
        if protocol_mismatch {
            return Self::VersionMismatch;
        }
        if stale_socket {
            return Self::StaleSocket;
        }
        Self::WontStart
    }

    /// Whether retry is offered on this failure surface.
    #[must_use]
    const fn is_retryable(self) -> bool {
        !matches!(self, Self::VersionMismatch)
    }

    #[must_use]
    const fn hints(self) -> &'static [(&'static str, &'static str)] {
        if self.is_retryable() {
            &[
                ("r", "retry"),
                ("L", "open log"),
                ("D", "run doctor"),
                ("ctrl-q", "quit"),
            ]
        } else {
            &[("D", "run doctor"), ("L", "open log"), ("ctrl-q", "quit")]
        }
    }
}

/// The headline each failure gets. §3.12 B fixes the first one verbatim.
#[must_use]
fn failure_headline(failure: DaemonFailure) -> &'static str {
    match failure {
        DaemonFailure::StaleSocket | DaemonFailure::WontStart => "fleetd could not start.",
        DaemonFailure::VersionMismatch => "fleetd speaks a different protocol.",
    }
}

/// The detail line under the headline.
#[must_use]
fn failure_detail(failure: DaemonFailure, message: &str, socket: &str) -> String {
    match failure {
        DaemonFailure::StaleSocket => format!("The socket {socket} is stale."),
        DaemonFailure::VersionMismatch => format!(
            "This app speaks protocol {APP_PROTOCOL}. Update whichever half is older; \
             reconnecting will not help. ({message})"
        ),
        DaemonFailure::WontStart => message.to_owned(),
    }
}

/// A synthetic doctor row for the version handshake, so the table answers the question the
/// mismatch surface raised.
#[must_use]
pub fn protocol_row(daemon_protocol: Option<u32>) -> DoctorRow {
    match daemon_protocol {
        Some(protocol) if protocol == APP_PROTOCOL => DoctorRow::new(
            "protocol",
            DoctorStatus::Ok,
            format!("app {APP_PROTOCOL} \u{00b7} fleetd {protocol}"),
        ),
        Some(protocol) => DoctorRow::new(
            "protocol",
            DoctorStatus::Fail,
            format!("app {APP_PROTOCOL} \u{00b7} fleetd {protocol}"),
        ),
        // Absence of knowledge never renders as good news (§1.3).
        None => DoctorRow::new(
            "protocol",
            DoctorStatus::Warn,
            format!("app {APP_PROTOCOL} \u{00b7} fleetd unknown"),
        ),
    }
}

mod retained;
pub(crate) use retained::DiagnosticView;

fn report_view(
    rows: &[DoctorRow],
    error: Option<&SharedString>,
    scroll: &gpui::ScrollHandle,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .size_full()
        .gap(theme.space.lg)
        .p(theme.space.xxl)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    Icon::Info
                        .el()
                        .size(IconSize::Medium)
                        .color(theme.colors.text_secondary),
                )
                .child(Text::title(TITLE)),
        )
        .child(
            div()
                .id("doctor-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(DoctorTable::new(rows.iter().cloned())),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.md)
                .children(error.map(|error| Text::ui(error.clone()).tone(Tone::Danger).ellipsize()))
                .child(div().flex_1())
                .child(
                    KeyHintRow::new()
                        .key("D", "re-run")
                        .key("L", "open log")
                        .key("esc", "close"),
                ),
        )
        .into_any_element()
}

fn failure_view(
    failure: DaemonFailure,
    detail: &SharedString,
    log_tail: &[SharedString],
    error: Option<&SharedString>,
    scroll: &gpui::ScrollHandle,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let mut hints = KeyHintRow::new();
    for (key, label) in failure.hints() {
        hints = hints.key(*key, *label);
    }
    div()
        .id("daemon-failure-body")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .min_h_full()
                .gap(theme.space.md)
                .child(
                    Icon::Unplug
                        .el()
                        .size(IconSize::Large)
                        .color(theme.colors.danger),
                )
                .child(Text::ui_strong(failure_headline(failure)).tone(Tone::Danger))
                .child(Text::ui(detail.clone()).muted())
                .children(error.map(|error| Text::ui(error.clone()).tone(Tone::Danger)))
                .child(
                    div().flex().flex_col().gap(theme.space.xxs).children(
                        log_tail
                            .iter()
                            .map(|line| Text::data_small(line.clone()).faint().ellipsize()),
                    ),
                )
                .child(hints),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
