//! The daemon-health surfaces of UX-SPEC §3.12: Doctor, and the three states that lead to it.
//!
//! **[D-16]: three *distinct* daemon situations, and only one of them is about reconnecting.**
//!
//! | | Situation | Surface |
//! | --- | --- | --- |
//! | A | cold start, fleetd not up yet | `Starting fleetd…`, full window, nothing bound |
//! | B | fleetd will not start | full window, the log tail, `r` / `L` / `D` / `ctrl-q` |
//! | C | fleetd died while attached | the 28 px banner, `r` / `l` / `Esc` |
//!
//! A **protocol version mismatch** is a fourth thing that looks like B and is not: retrying
//! cannot fix it, so the surface must say what actually has to happen (update one side) instead
//! of counting down a reconnect. [`DaemonFailure::classify`] separates the two from the error
//! text the client hands the shell, and [`failure_headline`] gives each its own sentence.
//!
//! Doctor itself renders `fleet doctor` as the compact table of §3.12: failures in red, `ok` in
//! the secondary tone — zero-suppression of good news at the color level.

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
pub const TITLE: &str = "Doctor";

// ---------------------------------------------------------------------------- doctor table

/// Maps one wire check onto a kit row.
///
#[must_use]
pub fn doctor_row(check: &DoctorCheck) -> DoctorRow {
    DoctorRow::new(
        check.check.clone(),
        match check.status {
            WireDoctorStatus::Ok => DoctorStatus::Ok,
            WireDoctorStatus::Warn => DoctorStatus::Warn,
            WireDoctorStatus::Fail => DoctorStatus::Fail,
        },
        check.detail.clone(),
    )
}

/// The whole table, in the daemon's order — the order is the diagnosis, so it is never sorted.
#[must_use]
pub fn doctor_rows(checks: &[DoctorCheck]) -> Vec<DoctorRow> {
    checks.iter().map(doctor_row).collect()
}

/// The checks that failed, as `"<check>: <detail>"` — the lines that belong in the sticky slot.
#[must_use]
pub fn failures(checks: &[DoctorCheck]) -> Vec<String> {
    checks
        .iter()
        .filter(|check| check.status == WireDoctorStatus::Fail)
        .map(|check| format!("{}: {}", check.check, check.detail))
        .collect()
}

/// Whether every check passed.
#[must_use]
pub fn is_healthy(checks: &[DoctorCheck]) -> bool {
    checks
        .iter()
        .all(|check| check.status != WireDoctorStatus::Fail)
}

/// The one-line summary of a doctor run: the first failure, or the all-clear.
///
/// The all-clear is a 1.6 s toast (§2.7 "instant-action acknowledgement"); a failure is sticky.
pub fn summary(checks: &[DoctorCheck]) -> Result<String, String> {
    match failures(checks).into_iter().next() {
        Some(first) => Err(first),
        None => Ok(format!("doctor: {} checks ok", checks.len())),
    }
}

// ---------------------------------------------------------------------------- failure kinds

/// Why the daemon is not usable, when the client could not establish a link (§3.12 B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonFailure {
    /// A stale `fleetd.sock` is the known cause; retrying after removing it works.
    StaleSocket,
    /// The daemon answered but speaks another protocol. **Retrying cannot fix this.**
    VersionMismatch,
    /// Anything else: the process did not come up.
    WontStart,
}

impl DaemonFailure {
    /// Classifies a `fleet-client` failure message.
    ///
    /// `Client::connect` reports a rejected handshake as `ConnectError::InvalidHandshake` /
    /// "rejected the handshake", and the daemon's own refusal reads `unsupported protocol N`.
    /// Both mean the same thing to the user, and neither is a crash.
    #[must_use]
    pub fn classify(message: &str, stale_socket: bool) -> Self {
        let lowered = message.to_lowercase();
        if lowered.contains("protocol")
            || lowered.contains("handshake")
            || lowered.contains("unsupported")
        {
            return Self::VersionMismatch;
        }
        if stale_socket {
            return Self::StaleSocket;
        }
        Self::WontStart
    }

    /// Whether `r` can plausibly help. A version mismatch is the one case where it cannot.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        !matches!(self, Self::VersionMismatch)
    }

    /// The keys the surface offers. `D` (doctor) is always there: it is how the user finds out
    /// *which* half is old.
    #[must_use]
    pub const fn hints(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::VersionMismatch => &[("D", "run doctor"), ("L", "open log"), ("ctrl-q", "quit")],
            Self::StaleSocket | Self::WontStart => &[
                ("r", "retry"),
                ("L", "open log"),
                ("D", "run doctor"),
                ("ctrl-q", "quit"),
            ],
        }
    }
}

/// The headline each failure gets. §3.12 B fixes the first one verbatim.
#[must_use]
pub fn failure_headline(failure: DaemonFailure) -> &'static str {
    match failure {
        DaemonFailure::StaleSocket | DaemonFailure::WontStart => "fleetd could not start.",
        DaemonFailure::VersionMismatch => "fleetd speaks a different protocol.",
    }
}

/// The detail line under the headline.
#[must_use]
pub fn failure_detail(failure: DaemonFailure, message: &str, socket: &str) -> String {
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

// ---------------------------------------------------------------------------- elements

/// The doctor table as a full surface, with the keys that leave it.
#[must_use]
pub fn view(checks: &[DoctorCheck], daemon_protocol: Option<u32>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut rows = vec![protocol_row(daemon_protocol)];
    rows.extend(doctor_rows(checks));
    let healthy = is_healthy(checks);

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
        .child(DoctorTable::new(rows))
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.md)
                .children((!healthy).then(|| {
                    Text::ui(SharedString::from(
                        summary(checks).err().unwrap_or_default(),
                    ))
                    .tone(Tone::Danger)
                    .ellipsize()
                }))
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

/// The §3.12 B surface for a link that cannot be established, including the mismatch variant.
#[must_use]
pub fn failure_view(
    failure: DaemonFailure,
    message: &str,
    socket: &str,
    log_tail: &[String],
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let mut hints = KeyHintRow::new();
    for (key, label) in failure.hints() {
        hints = hints.key(*key, *label);
    }
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .gap(theme.space.md)
        .child(
            Icon::Unplug
                .el()
                .size(IconSize::Large)
                .color(theme.colors.danger),
        )
        .child(Text::ui_strong(failure_headline(failure)).tone(Tone::Danger))
        .child(Text::ui(SharedString::from(failure_detail(failure, message, socket))).muted())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.space.xxs)
                .children(log_tail.iter().map(|line| {
                    Text::data_small(SharedString::from(line.clone()))
                        .faint()
                        .ellipsize()
                })),
        )
        .child(hints)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, ok: bool, detail: &str) -> DoctorCheck {
        DoctorCheck {
            check: name.to_owned(),
            status: if ok {
                WireDoctorStatus::Ok
            } else {
                WireDoctorStatus::Fail
            },
            detail: detail.to_owned(),
        }
    }

    #[test]
    fn a_failure_renders_red_and_an_ok_stays_neutral() {
        let checks = vec![
            check("git", true, "git version 2.49.0"),
            check("gh auth", false, "gh: not logged in to github.com"),
        ];
        let rows = doctor_rows(&checks);
        assert_eq!(rows[0].status, DoctorStatus::Ok);
        assert_eq!(
            rows[0].status.tone(),
            Tone::Secondary,
            "good news never gets a hue"
        );
        assert_eq!(rows[1].status, DoctorStatus::Fail);
        assert_eq!(rows[1].status.tone(), Tone::Danger);
        assert_eq!(rows.len(), checks.len(), "the daemon's order is preserved");
    }

    #[test]
    fn the_summary_leads_with_the_first_failure() {
        let checks = vec![
            check("git", true, "ok"),
            check("gh auth", false, "gh: not logged in to github.com"),
            check("host devbox", false, "ssh: connect timed out after 5s"),
        ];
        assert_eq!(
            summary(&checks),
            Err("gh auth: gh: not logged in to github.com".to_owned())
        );
        assert_eq!(failures(&checks).len(), 2);
        assert!(!is_healthy(&checks));

        let healthy = vec![check("git", true, "ok")];
        assert_eq!(summary(&healthy), Ok("doctor: 1 checks ok".to_owned()));
        assert!(is_healthy(&healthy));
        assert!(is_healthy(&[]));
    }

    #[test]
    fn a_version_mismatch_is_not_a_crash_and_never_offers_retry() {
        let mismatch = DaemonFailure::classify(
            "Fleet daemon rejected the handshake: unsupported protocol 2; expected 1",
            false,
        );
        assert_eq!(mismatch, DaemonFailure::VersionMismatch);
        assert!(!mismatch.is_retryable());
        assert!(
            !mismatch.hints().iter().any(|(key, _)| *key == "r"),
            "offering a retry that cannot work is worse than offering none"
        );
        assert!(failure_headline(mismatch).contains("protocol"));
        assert!(
            failure_detail(mismatch, "unsupported protocol 2", "/tmp/s.sock")
                .contains("reconnecting will not help")
        );
    }

    #[test]
    fn a_stale_socket_keeps_its_own_sentence() {
        let stale = DaemonFailure::classify("could not connect to Fleet daemon", true);
        assert_eq!(stale, DaemonFailure::StaleSocket);
        assert!(stale.is_retryable());
        assert_eq!(
            failure_detail(stale, "irrelevant", "~/.fleet/fleetd.sock"),
            "The socket ~/.fleet/fleetd.sock is stale."
        );
        assert_eq!(failure_headline(stale), "fleetd could not start.");
    }

    #[test]
    fn anything_else_is_a_plain_failure_that_quotes_the_daemon() {
        let wont_start =
            DaemonFailure::classify("Fleet daemon exited before becoming ready", false);
        assert_eq!(wont_start, DaemonFailure::WontStart);
        assert!(wont_start.is_retryable());
        assert_eq!(
            failure_detail(wont_start, "exited with status 1", "/tmp/s.sock"),
            "exited with status 1"
        );
    }

    #[test]
    fn an_unknown_daemon_protocol_is_amber_never_ok() {
        assert_eq!(protocol_row(Some(APP_PROTOCOL)).status, DoctorStatus::Ok);
        assert_eq!(
            protocol_row(Some(APP_PROTOCOL.saturating_add(1))).status,
            DoctorStatus::Fail
        );
        assert_eq!(
            protocol_row(None).status,
            DoctorStatus::Warn,
            "absence of knowledge never renders as good news"
        );
    }
}
