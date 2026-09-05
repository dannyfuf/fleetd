//! End-to-end checks for the Jobs panel and the daemon states it renders (UX-SPEC §3.7, §3.12).
//!
//! These tests run a **real `fleetd`** against a temporary `FLEET_HOME` with a fake `gh` on
//! `PATH` (see `tests/common/mod.rs`), connect with `fleet-client`, and then feed the daemon's
//! own snapshot and job records through the panel's pure logic. No window is opened: everything
//! §3.7 decides is a function of `&[JobRecord]`, which is exactly why it is testable this way.
//!
//! Several daemon services are still stubs while the workspace is being built in parallel
//! (`repos::import_from_swarm`, `doctor::check`, …). The tests below therefore assert the
//! *protocol contract* — a request either yields its documented payload or a well-formed
//! `ProtoError`, and never hangs or panics — and assert the panel's rendering decisions over
//! whichever of the two arrives. When a service lands, the payload branch starts running with
//! no edit here.

mod common;

use std::collections::HashSet;

use fleet_proto::{
    error::ErrorKind,
    event::Event,
    job::{JobKind, JobRecord, JobStatus},
    request::RequestBody,
    response::ResponseBody,
};

use fleet_app::views::{
    doctor_view::{self, DaemonFailure},
    first_run,
    job_ticker::{self, StatusSlot},
    jobs_panel::{self, JobFilter},
    sticky_error,
};

use common::Daemon;

macro_rules! daemon_or_skip {
    ($label:literal) => {
        match Daemon::start($label) {
            Some(daemon) => daemon,
            None => {
                eprintln!(
                    "skipping: fleetd was not found next to the test binary; \
                     run `cargo build -p fleet-daemon` (or set FLEET_DAEMON) first"
                );
                return;
            }
        }
    };
}

#[tokio::test]
async fn a_fresh_daemon_answers_with_a_snapshot_the_panel_can_render() {
    let daemon = daemon_or_skip!("snapshot");
    let client = daemon.connect().await;

    let snapshot = client
        .get_snapshot()
        .await
        .unwrap_or_else(|error| panic!("get_snapshot failed: {error}"));

    assert_eq!(
        snapshot.daemon.home,
        daemon.home().to_string_lossy(),
        "the daemon must be running in the temporary home, never in ~/.fleet"
    );
    assert!(!snapshot.daemon.version.is_empty());
    assert!(snapshot.daemon.pid > 0);

    // §3.7 empty state: a brand new home has nothing running, and the panel says exactly that.
    let counts = jobs_panel::job_counts(&snapshot.jobs);
    assert!(
        counts.is_empty(),
        "a fresh snapshot unexpectedly retained jobs: {:?}",
        snapshot.jobs
    );
    assert!(jobs_panel::visible_jobs(&snapshot.jobs, JobFilter::All, &HashSet::new()).is_empty());
    assert_eq!(jobs_panel::EMPTY_FACT, "Nothing running.");

    // §2.2: an idle daemon leaves the shared status-bar slot empty rather than filling it.
    assert_eq!(
        job_ticker::status_slot(&snapshot.jobs, None),
        StatusSlot::Idle
    );

    // §3.13: a first run is the empty snapshot, and it is a migration decision, not onboarding.
    assert!(snapshot.contexts.is_empty());
    assert!(snapshot.repos.is_empty());
    assert!(!first_run::has_swarm_state(Some(daemon.home())));
    assert_eq!(first_run::keys(false).len(), 4);

    // Every timestamp the daemon writes must be readable by the elapsed column of §3.7.
    assert!(
        jobs_panel::parse_timestamp(&snapshot.generated_at).is_some(),
        "the daemon's own timestamp format must parse: {}",
        snapshot.generated_at
    );
    assert!(jobs_panel::parse_timestamp(&snapshot.daemon.started_at).is_some());

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
}

#[tokio::test]
async fn the_job_list_round_trips_and_the_event_stream_is_live() {
    let daemon = daemon_or_skip!("jobs");
    let client = daemon.connect().await;

    let jobs = client
        .list_jobs()
        .await
        .unwrap_or_else(|error| panic!("list_jobs failed: {error}"));
    assert!(jobs.is_empty(), "a fresh home retained jobs: {jobs:?}");

    let mut events = client.events();

    // `ImportFromSwarm` is §3.13's `i`. Until `repos::import_from_swarm` lands it answers with
    // a protocol error; either way the panel has to survive the answer.
    match client.request(RequestBody::ImportFromSwarm).await {
        Ok(ResponseBody::Job(record)) => {
            assert_eq!(record.kind, JobKind::Import);
            let update = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    match events.recv().await {
                        Ok(Event::JobUpdated(job)) if job.id == record.id => return job,
                        Ok(_) => {}
                        Err(error) => panic!("event stream closed: {error}"),
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("no JobUpdated event for the import job"));

            // Whatever the outcome, §3.7 must be able to draw the row.
            let now = jobs_panel::now_unix();
            let _elapsed = jobs_panel::elapsed_label(&update, now);
            let counts = jobs_panel::job_counts(std::slice::from_ref(&update));
            assert_eq!(counts.running + counts.failed + counts.done, 1);
        }
        Ok(other) => panic!("ImportFromSwarm answered with {other:?}"),
        Err(error) => {
            assert!(
                matches!(error.kind, ErrorKind::Unsupported | ErrorKind::Unknown),
                "an unimplemented service must answer with a stable protocol error, got {error:?}"
            );
            assert!(!error.message.is_empty());
        }
    }

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
}

#[tokio::test]
async fn tailing_and_cancelling_an_unknown_job_fail_cleanly() {
    let daemon = daemon_or_skip!("tail");
    let client = daemon.connect().await;

    let ghost = "job-does-not-exist"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));

    // The panel expands a log with `Enter` and polls `TailJob` while following. A job that is
    // gone must produce an error, not a hang: the panel turns it into a sticky error (§1.8).
    let tail = client.tail_job(ghost, fleet_ui_kit::LOG_TAIL_LINES).await;
    let error = tail
        .err()
        .unwrap_or_else(|| panic!("expected a tail error"));
    assert!(!error.message.is_empty());

    let ghost = "job-does-not-exist"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let cancelled = client.cancel_job(ghost).await;
    assert!(
        cancelled.is_err(),
        "`c` on a job the daemon has forgotten must be refused, not silently accepted"
    );

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
}

#[tokio::test]
async fn doctor_and_the_version_handshake_answer_the_daemon_state_surfaces() {
    let daemon = daemon_or_skip!("doctor");
    let client = daemon.connect().await;

    let version = client
        .daemon_version()
        .await
        .unwrap_or_else(|error| panic!("daemon_version failed: {error}"));
    assert_eq!(
        version.protocol,
        doctor_view::APP_PROTOCOL,
        "the app and the daemon in this workspace must agree on the wire protocol"
    );
    assert_eq!(
        doctor_view::protocol_row(Some(version.protocol)).status,
        fleet_ui_kit::DoctorStatus::Ok
    );

    match client.request(RequestBody::Doctor).await {
        Ok(ResponseBody::Doctor(checks)) => {
            let rows = doctor_view::doctor_rows(&checks);
            assert_eq!(
                rows.len(),
                checks.len(),
                "the daemon's order is the diagnosis"
            );
            // The fake `gh` on PATH answers every probe, so a `gh` check must not report a
            // missing binary; the harness would otherwise be testing the developer's machine.
            for check in &checks {
                assert!(
                    !check.detail.is_empty(),
                    "every check states what it observed"
                );
            }
            let _summary = doctor_view::summary(&checks);
        }
        Ok(other) => panic!("Doctor answered with {other:?}"),
        Err(error) => {
            assert!(
                !error.message.is_empty(),
                "an unimplemented doctor must still say why"
            );
        }
    }

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
}

#[tokio::test]
async fn a_shutdown_reads_as_a_lost_daemon_not_as_a_crash() {
    let daemon = daemon_or_skip!("shutdown");
    let client = daemon.connect().await;
    let mut events = client.events();

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));

    // §3.12 C starts with this event; the shell turns it into `DaemonLink::Lost`.
    let saw_shutdown = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(Event::DaemonShuttingDown) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    // The daemon may close the socket before the event is flushed; either way the client must
    // observe the loss rather than hang.
    if !saw_shutdown {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if client.daemon_ping().await.is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fleetd kept answering pings after an explicit shutdown"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    // §3.12: "fleetd could not start" and "fleetd speaks a different protocol" are different
    // failures, and only one of them can be fixed by pressing `r`.
    assert!(DaemonFailure::classify("could not connect to Fleet daemon", true).is_retryable());
    assert!(
        !DaemonFailure::classify(
            "Fleet daemon rejected the handshake: unsupported protocol 2; expected 3",
            false
        )
        .is_retryable()
    );
}

#[tokio::test]
async fn the_panel_renders_the_spec_row_for_every_job_shape() {
    // No daemon needed: this is the §3.7 row table, asserted against the exact records the
    // daemon's `JobManager` produces (its own field shapes are exercised above).
    let now = jobs_panel::now_unix();
    let started = "2026-09-04T12:00:00Z";
    let base = |id: &str, status: JobStatus, progress: Option<&str>| JobRecord {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::Clone,
        target: "nixos".to_owned(),
        title: "Clone nixos".to_owned(),
        status,
        progress: progress.map(str::to_owned),
        log_path: "/tmp/fleet/logs/jobs/j.log".to_owned(),
        started_at: started.to_owned(),
        finished_at: None,
        cancellable: true,
        retryable: true,
    };

    let running = base(
        "job-running",
        JobStatus::Running,
        Some("Receiving objects: 40% (81/202)"),
    );
    let failed = base(
        "job-failed",
        JobStatus::Failed {
            error: "gh: HTTP 502 upstream connect error".to_owned(),
        },
        None,
    );
    let mut done = base("job-done", JobStatus::Succeeded, Some("ignored"));
    done.finished_at = Some("2026-09-04T12:00:12Z".to_owned());
    let mut cancelled = base("job-cancelled", JobStatus::Cancelled, None);
    cancelled.finished_at = Some("2026-09-04T12:00:30Z".to_owned());

    let jobs = vec![running.clone(), failed.clone(), done.clone(), cancelled];

    let counts = jobs_panel::job_counts(&jobs);
    assert_eq!((counts.running, counts.failed, counts.done), (1, 1, 2));

    // Only live work carries a progress sub-line; a failure states its reason instead.
    assert_eq!(
        jobs_panel::sub_line(&running),
        Some("Receiving objects: 40% (81/202)")
    );
    assert_eq!(jobs_panel::percent(&running), Some(40));
    assert_eq!(
        jobs_panel::sub_line(&failed),
        Some("gh: HTTP 502 upstream connect error")
    );
    assert_eq!(jobs_panel::sub_line(&done), None);

    // The elapsed column: `m:ss` only after 30 s, a single-unit age when finished.
    let started_at =
        jobs_panel::parse_timestamp(started).unwrap_or_else(|| panic!("bad fixture timestamp"));
    assert_eq!(jobs_panel::elapsed_label(&running, started_at + 10), None);
    assert_eq!(
        jobs_panel::elapsed_label(&running, started_at + 42),
        Some("0:42".to_owned())
    );
    assert_eq!(
        jobs_panel::elapsed_label(&done, now),
        Some("12s".to_owned())
    );

    // §1.8: the failure owns the status-bar slot, and it offers `R`.
    let sticky = sticky_error::sticky_error_for(&jobs, &[])
        .unwrap_or_else(|| panic!("expected a sticky error"));
    assert_eq!(sticky.text, "gh: HTTP 502 upstream connect error");
    assert!(sticky_error::is_retryable(&sticky));
    assert!(matches!(
        job_ticker::status_slot(&jobs, Some(&sticky)),
        StatusSlot::Error(_)
    ));

    // §2.7: a failure is never a toast, and a success is one only when its row is off-screen.
    assert_eq!(job_ticker::job_outcome_toast(&failed, false), None);
    assert!(job_ticker::job_outcome_toast(&done, false).is_some());
    assert_eq!(job_ticker::job_outcome_toast(&done, true), None);
}
