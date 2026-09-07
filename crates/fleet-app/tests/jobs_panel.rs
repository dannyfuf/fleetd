//! Real daemon protocol and job progression used by the Jobs panel.

mod common;

use std::collections::HashSet;

use fleet_proto::{
    event::Event,
    job::{JobKind, JobStatus},
    request::RequestBody,
    response::ResponseBody,
};

use fleet_app::presentation::{is_active, parse_timestamp, sub_line};
use fleet_app::views::{
    doctor_view, first_run,
    job_ticker::{self, StatusSlot},
    jobs_panel::{self, JobFilter},
};

use common::Daemon;

#[tokio::test]
async fn a_fresh_daemon_answers_with_a_snapshot_the_panel_can_render() {
    let daemon = Daemon::start("snapshot").expect("start isolated fleetd");
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
        parse_timestamp(&snapshot.generated_at).is_some(),
        "the daemon's own timestamp format must parse: {}",
        snapshot.generated_at
    );
    assert!(parse_timestamp(&snapshot.daemon.started_at).is_some());

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
}

#[tokio::test]
async fn the_job_list_round_trips_and_the_event_stream_is_live() {
    let daemon = Daemon::start("jobs").expect("start isolated fleetd");
    let client = daemon.connect().await;

    let jobs = client
        .list_jobs()
        .await
        .unwrap_or_else(|error| panic!("list_jobs failed: {error}"));
    assert!(jobs.is_empty(), "a fresh home retained jobs: {jobs:?}");

    let mut events = client.events();

    // A missing source is a deterministic job failure, not an unsupported service.
    let ResponseBody::Job(record) = client
        .request(RequestBody::ImportFromSwarm)
        .await
        .expect("start import job")
    else {
        panic!("ImportFromSwarm must return a job");
    };
    assert_eq!(record.kind, JobKind::Import);
    let completed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Ok(Event::JobUpdated(job)) if job.id == record.id && !is_active(&job.status) => {
                    return job;
                }
                Ok(_) => {}
                Err(error) => panic!("import event stream closed: {error}"),
            }
        }
    })
    .await
    .expect("import job must complete");
    let JobStatus::Failed { error } = &completed.status else {
        panic!("missing swarm files must fail the import: {completed:?}");
    };
    assert!(!error.is_empty());
    assert_eq!(sub_line(&completed), Some(error.as_str()));
    let retained = client.list_jobs().await.expect("read completed jobs");
    assert!(
        retained
            .iter()
            .any(|job| job.id == completed.id && job.status == completed.status)
    );
    let tail = client
        .tail_job(completed.id.clone(), fleet_ui_kit::LOG_TAIL_LINES)
        .await
        .expect("read import log");
    assert!(
        !tail.is_empty(),
        "failed import must retain diagnostic output"
    );

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
}

#[tokio::test]
async fn tailing_and_cancelling_an_unknown_job_fail_cleanly() {
    let daemon = Daemon::start("tail").expect("start isolated fleetd");
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
    let daemon = Daemon::start("doctor").expect("start isolated fleetd");
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

    let ResponseBody::Doctor(checks) = client
        .request(RequestBody::Doctor)
        .await
        .expect("doctor is implemented")
    else {
        panic!("Doctor must return checks");
    };
    assert!(!checks.is_empty());
    assert_eq!(doctor_view::doctor_rows(&checks).len(), checks.len());
    assert!(checks.iter().all(|check| !check.detail.is_empty()));
    let github = checks
        .iter()
        .find(|check| check.check == "gh auth")
        .expect("doctor checks gh authentication");
    assert_eq!(
        github.status,
        fleet_proto::response::DoctorStatus::Ok,
        "fake gh must be authenticated"
    );

    client
        .daemon_shutdown(true)
        .await
        .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
}

#[tokio::test]
async fn a_shutdown_reads_as_a_lost_daemon_not_as_a_crash() {
    let daemon = Daemon::start("shutdown").expect("start isolated fleetd");
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
}
