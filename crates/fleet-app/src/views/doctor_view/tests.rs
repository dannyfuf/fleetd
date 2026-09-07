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

    let healthy = vec![check("git", true, "ok")];
    assert_eq!(summary(&healthy), Ok("doctor: 1 checks ok".to_owned()));
}

#[test]
fn words_do_not_fake_protocol_mismatch() {
    let prose = "a plugin handshake is unsupported by this host";
    let now = std::time::Instant::now();
    let mut state = crate::state::AppState::new("/tmp/fleet", now);
    state.apply_bridge_event(
        crate::bridge::BridgeEvent::ConnectFailed {
            message: prose.to_owned(),
            log_tail: Vec::new(),
            stale_socket: false,
        },
        now,
    );
    let crate::state::DaemonLink::Failed {
        protocol_mismatch, ..
    } = &state.daemon
    else {
        panic!("connection failure must use the daemon failure surface");
    };
    assert_eq!(
        DaemonFailure::classify(*protocol_mismatch, false),
        DaemonFailure::WontStart
    );

    state.apply_bridge_event(
        crate::bridge::BridgeEvent::ProtocolMismatch {
            message: "unsupported protocol 2; expected 4".to_owned(),
            log_tail: Vec::new(),
        },
        now,
    );
    let crate::state::DaemonLink::Failed {
        message,
        protocol_mismatch,
        ..
    } = &state.daemon
    else {
        panic!("protocol mismatch must use the daemon failure surface");
    };
    assert!(*protocol_mismatch);
    assert_eq!(message, "unsupported protocol 2; expected 4");
    let mismatch = DaemonFailure::classify(*protocol_mismatch, false);
    assert_eq!(mismatch, DaemonFailure::VersionMismatch);
    assert!(
        !mismatch.hints().iter().any(|(key, _)| *key == "r"),
        "offering a retry that cannot work is worse than offering none"
    );
    assert!(failure_headline(mismatch).contains("protocol"));
    assert!(
        failure_detail(mismatch, message, "/tmp/s.sock").contains("reconnecting will not help")
    );
}

#[test]
fn a_stale_socket_keeps_its_own_sentence() {
    let stale = DaemonFailure::classify(false, true);
    assert_eq!(stale, DaemonFailure::StaleSocket);
    assert_eq!(
        failure_detail(stale, "irrelevant", "~/.fleet/fleetd.sock"),
        "The socket ~/.fleet/fleetd.sock is stale."
    );
    assert_eq!(failure_headline(stale), "fleetd could not start.");
}

#[test]
fn anything_else_is_a_plain_failure_that_quotes_the_daemon() {
    let wont_start = DaemonFailure::classify(false, false);
    assert_eq!(wont_start, DaemonFailure::WontStart);
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
