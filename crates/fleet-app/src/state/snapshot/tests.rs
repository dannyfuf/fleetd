use super::*;
use crate::state::test_support::*;

#[test]
fn breadcrumb_suppresses_empty_parts() {
    assert_eq!(
        breadcrumb(&["buk", "payroll", "feat/payroll-fix"]),
        "buk › payroll › feat/payroll-fix"
    );
    assert_eq!(breadcrumb(&["buk", "", "row"]), "buk › row");
    assert_eq!(breadcrumb(&[]), "");
}

#[test]
fn chip_counts_never_collapse_unknown_into_another_chip() {
    let mut snapshot = snapshot();
    snapshot.statuses = [
        SessionState::Attached,
        SessionState::Attached,
        SessionState::Detached,
        SessionState::Unknown,
        SessionState::None,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, session)| fleet_core::sessions::WorktreeStatus {
        worktree_id: format!("buk/payroll#feat-{index}")
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        session,
        windows: Vec::new(),
        running: Vec::new(),
        agent_activity: AgentActivity::Unknown,
        agent_activity_changed_at: None,
    })
    .collect();
    snapshot.hosts = vec![fleet_proto::snapshot::HostStatus {
        id: "devbox".parse().unwrap_or_else(|error| panic!("{error}")),
        reachable: false,
        checked_at: "2026-09-04T12:00:00Z".to_owned(),
        error: None,
    }];

    let counts = ChipCounts::from_snapshot(&snapshot, 4);

    assert_eq!(counts.live, 2);
    assert_eq!(counts.sleeping, 1);
    assert_eq!(counts.unknown, 2, "one unknown session plus one dead host");
    assert_eq!(counts.review, 4);
}

#[test]
fn first_run_never_reappears_after_a_populated_snapshot() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let mut populated = snapshot();
    populated.contexts.push(fleet_core::model::Context {
        id: "alpha".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Alpha".to_owned(),
        owners: vec!["acme".to_owned()],
        created_at: "2026-09-05T00:00:00Z".to_owned(),
    });
    state.apply_snapshot(populated, now);
    assert!(!state.is_first_run());

    state.apply_snapshot(snapshot(), now);

    assert!(!state.is_first_run());
    assert_ne!(state.context_chain(), vec!["FirstRun"]);
}

#[test]
fn a_snapshot_without_a_terminal_frees_its_mirror_and_its_mru_entry() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let session: SessionId = "payroll/feat".parse().unwrap_or_else(|e| panic!("{e}"));

    let mut open = snapshot();
    open.sessions = vec![session_with("payroll/feat", &[1, 2])];
    state.apply_snapshot(open, now);
    state.apply_frame(&frame(1, true, vec![row(0, "one")]));
    let mut second = frame(1, true, vec![row(0, "two")]);
    second.terminal = TerminalId(2);
    state.apply_frame(&second);
    state.touch_terminal(&session, TerminalId(1));
    state.touch_terminal(&session, TerminalId(2));
    assert_eq!(state.grids.len(), 2);

    // `ctrl-s x` on terminal 2: the daemon drops it from the session, and nothing can ever
    // paint its 12 000 cells again.
    let mut closed = snapshot();
    closed.sessions = vec![session_with("payroll/feat", &[1])];
    state.apply_snapshot(closed, now);
    assert_eq!(state.grids.keys().collect::<Vec<_>>(), vec![&TerminalId(1)]);
    assert_eq!(
        state.terminal_mru.get(&session).map(Mru::entries),
        Some(&vec![TerminalId(1)][..])
    );

    // Killing the session frees the rest, MRU included.
    state.apply_snapshot(snapshot(), now);
    assert!(state.grids.is_empty());
    assert!(state.terminal_mru.is_empty());
}

#[test]
fn acknowledged_failures_are_reconciled_to_snapshot_membership() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let mut snapshot = snapshot();
    let failure = job(
        "failed",
        JobStatus::Failed {
            error: "failure".into(),
        },
        None,
    );
    snapshot.jobs.push(failure.clone());
    state.apply_snapshot(snapshot.clone(), now);
    state.open_overlay(Overlay::Jobs);
    assert!(state.seen_failed.contains(&failure.id));
    assert!(state.sticky_error.is_none());
    state.apply_snapshot(snapshot, now);
    assert!(state.seen_failed.contains(&failure.id));
    state.apply_snapshot(crate::state::test_support::snapshot(), now);
    assert!(state.seen_failed.is_empty());
}

#[test]
fn snapshot_does_not_erase_explicit_error() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_toast_event(ToastLevel::Error, "explicit failure".to_owned(), now);

    state.apply_snapshot(snapshot(), now);
    state.apply_job(job("done", JobStatus::Succeeded, None), now);
    state.open_overlay(Overlay::Jobs);

    assert_eq!(
        state.sticky_error.as_ref().map(|error| error.text.as_str()),
        Some("explicit failure")
    );
}

#[test]
fn vanished_sessions_leave_session_mru() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let live: SessionId = "payroll/live"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let vanished: SessionId = "payroll/vanished"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    state.session_mru.touch(vanished.clone());
    state.session_mru.touch(live.clone());
    let mut current = snapshot();
    current.sessions = vec![session_with(live.as_str(), &[1])];

    state.apply_snapshot(current, now);

    assert_eq!(state.session_mru.entries(), &[live]);
}
