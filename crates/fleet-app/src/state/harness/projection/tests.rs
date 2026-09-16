use std::time::Instant;

use fleet_core::ids::TerminalId;
use fleet_proto::job::JobStatus;

use super::super::tests::{job, state};
use crate::dialogs::Dialogs;
use crate::state::{JobFilter, JobsPanelMirror, Overlay, Screen, test_support};

#[test]
fn jobs_panel_cursor_changes_focus_and_selected_row() {
    let now = Instant::now();
    let mut state = state();
    let mut snapshot = test_support::snapshot();
    snapshot.jobs.push(job("job-1", JobStatus::Running));
    snapshot.jobs.push(job("job-2", JobStatus::Succeeded));
    state.apply_snapshot(snapshot, now);
    state.open_overlay(Overlay::Jobs);

    let before = state.harness_projection();
    assert_eq!(before.snapshot.focused.as_deref(), Some("jobs.row[0]"));
    assert_eq!(
        before
            .snapshot
            .lists
            .get("jobs")
            .and_then(|jobs| jobs.selected.as_ref())
            .map(|row| row.id.as_str()),
        Some("job-1")
    );

    assert!(state.set_jobs_panel(JobsPanelMirror {
        cursor: 1,
        filter: JobFilter::All,
    }));
    let after = state.harness_projection();

    assert_eq!(after.snapshot.focused.as_deref(), Some("jobs.row[1]"));
    assert_eq!(
        after
            .snapshot
            .lists
            .get("jobs")
            .and_then(|jobs| jobs.selected.as_ref())
            .map(|row| row.id.as_str()),
        Some("job-2")
    );
    assert!(after.revision > before.revision);
}

#[test]
fn jobs_panel_filter_rebases_the_projected_rows() {
    let now = Instant::now();
    let mut state = state();
    let mut snapshot = test_support::snapshot();
    snapshot.jobs.push(job("job-1", JobStatus::Running));
    snapshot.jobs.push(job(
        "job-2",
        JobStatus::Failed {
            error: "boom".to_owned(),
        },
    ));
    snapshot.jobs.push(job("job-3", JobStatus::Succeeded));
    state.apply_snapshot(snapshot, now);
    state.open_overlay(Overlay::Jobs);
    let all = state.harness_projection().snapshot;
    let all_jobs = all.lists.get("jobs").expect("jobs list");
    assert_eq!(all_jobs.rows.len(), 3);
    assert_eq!(all_jobs.filter, "");

    assert!(state.set_jobs_panel(JobsPanelMirror {
        cursor: 0,
        filter: JobFilter::Failed,
    }));
    let dump = state.harness_projection().snapshot;
    let jobs = dump.lists.get("jobs").expect("jobs list");

    assert_eq!(dump.focused.as_deref(), Some("jobs.row[0]"));
    assert_eq!(jobs.filter, "failed");
    assert_eq!(
        jobs.rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["job-2"]
    );
    assert_eq!(
        jobs.selected.as_ref().map(|row| row.id.as_str()),
        jobs.rows.first().map(|row| row.id.as_str()),
        "jobs.row[0] and lists.jobs.rows[0] address the same filtered job"
    );
}

#[test]
fn rename_only_change_rebuilds_the_projection() {
    let now = Instant::now();
    let mut state = state();
    let session_id = "buk/payroll#feat";
    let mut snapshot = test_support::snapshot();
    let mut session = test_support::session_with(session_id, &[1]);
    session.terminals[0].title = Some("shell title".to_owned());
    snapshot.sessions.push(session);
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: session_id.parse().unwrap_or_else(|error| panic!("{error}")),
    };

    let before = state.harness_projection();
    assert_eq!(
        before
            .snapshot
            .lists
            .get("tabs")
            .and_then(|tabs| tabs.rows.first())
            .map(|row| row.label.as_str()),
        Some("shell title")
    );
    state.renamed_terminals.insert(TerminalId(1));
    let after = state.harness_projection();

    assert_eq!(state.harness_builds(), 2);
    assert_eq!(
        after
            .snapshot
            .lists
            .get("tabs")
            .and_then(|tabs| tabs.rows.first())
            .map(|row| row.label.as_str()),
        Some("t1")
    );
    assert!(after.revision > before.revision);
}

#[test]
fn settle_expiry_rebuilds_a_cached_busy_projection() {
    let state = state();
    let generation = state.harness.settle().begin(None);
    let busy = state.harness_projection();

    assert_eq!(busy.snapshot.idle.settling_mutations, 1);
    assert!(!busy.snapshot.idle.idle);
    assert!(state.harness.settle().expire(generation));

    let idle = state.harness_projection();
    assert_eq!(state.harness_builds(), 2);
    assert_eq!(idle.snapshot.idle.settling_mutations, 0);
    assert!(idle.snapshot.idle.link_opening);
    assert!(idle.revision > busy.revision);
}

#[test]
fn open_dialog_keeps_unmirrored_fields_and_message_empty() {
    let mut state = state();
    state.open_overlay(Overlay::Dialog(Dialogs::CreateWorktree));

    let dump = state.harness_projection().snapshot;
    let dialog = dump.dialog.as_ref().expect("open dialog snapshot");

    assert!(dialog.fields.is_empty());
    assert!(dialog.message.is_none());
}
