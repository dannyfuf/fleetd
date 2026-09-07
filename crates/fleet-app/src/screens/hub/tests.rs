use fleet_core::{ids::RepoId, inspection::WorktreeInspection, sessions::SessionState};
use fleet_proto::snapshot::Snapshot;

use super::*;

fn inspection(dirty: bool, unique: Option<u64>, merged: bool) -> WorktreeInspection {
    WorktreeInspection {
        worktree_id: WorktreeId::try_from("buk/payroll#a")
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        host: "local".to_owned(),
        path: "/tmp/wt".to_owned(),
        branch: "feat/x".to_owned(),
        base_ref: "origin/main".to_owned(),
        head: Some("abc".to_owned()),
        target_branch: "main".to_owned(),
        upstream: None,
        ahead: None,
        behind: None,
        upstream_gone: false,
        dirty,
        dirty_files: dirty.then_some(12),
        merged_into_target: merged,
        unique_commits: unique,
        published: true,
        merged,
        pr: None,
        session: SessionState::None,
        running: Vec::new(),
        inspected_at: "2026-09-04T11:59:00Z".to_owned(),
        warnings: Vec::new(),
        error: None,
    }
}

/// The default theme metrics, which is what the documented §2.1 frame is measured against.
fn pane_ch(width: f32, rail_collapsed: bool, detail_open: bool) -> f32 {
    list_pane_ch(
        width,
        rail_collapsed,
        detail_open,
        &fleet_ui_kit::theme::Metrics::default(),
    )
}

#[test]
fn the_ladder_matches_the_documented_frame() {
    // §2.1: 1280 px gives the list 138 ch, and 93 ch with the detail panel inset.
    assert!((pane_ch(1280.0, false, false) - 138.6).abs() < 0.2);
    assert!((pane_ch(1280.0, true, false) - 164.8).abs() < 0.2);
    assert!((pane_ch(1280.0, false, true) - 93.3).abs() < 0.2);
}

#[test]
fn a_narrow_window_docks_the_panel_instead_of_stealing_width() {
    assert!(detail_is_docked(1100.0));
    assert!(!detail_is_docked(1280.0));
    // Docked, the list keeps its full width.
    let docked = pane_ch(1100.0, false, true);
    assert!((docked - pane_ch(1100.0, false, false)).abs() < f32::EPSILON);
    assert!(docked > 72.0, "never below the 72 ch floor of §3.4");
}

#[test]
fn the_pr_cache_expires_on_the_documented_ttl() {
    let mut cache = PrCache::default();
    assert!(cache.needs_fetch(Instant::now()));
    assert!(cache.is_cold());
    let now = Instant::now();
    cache.fetched_at = Some(now);
    assert!(!cache.needs_fetch(now));
    assert!(!cache.needs_fetch(now + PR_TTL - Duration::from_secs(1)));
    assert!(cache.needs_fetch(now + PR_TTL));
    cache.loading = true;
    cache.started_at = Some(now);
    cache.fetched_at = None;
    assert!(!cache.needs_fetch(now), "a running fetch is not repeated");
    assert!(
        cache.needs_fetch(now + PR_TTL),
        "a fetch whose answer never arrived stops blocking the screen"
    );
}

#[test]
fn an_inspection_slot_never_blanks_its_previous_values() {
    let mut slot = Inspected::ready(inspection(true, Some(1), false));
    slot.loading = true;
    assert!(slot.data.is_some());
    assert_eq!(
        Inspected::failed("gh unavailable").error.as_deref(),
        Some("gh unavailable")
    );
}

fn snapshot(count: usize) -> Snapshot {
    Snapshot {
        generated_at: String::new(),
        contexts: vec![],
        repos: vec![],
        clones: vec![],
        worktrees: (0..count)
            .map(|index| Worktree {
                id: format!("acme/api#branch-{index}")
                    .parse()
                    .expect("worktree id"),
                repo_id: "acme/api".parse().expect("repo id"),
                slug: format!("branch-{index}"),
                branch: format!("branch-{index}"),
                base_ref: "main".into(),
                path: "/tmp/worktree".into(),
                session: format!("acme/api#branch-{index}"),
                host: None,
                created_at: "2026-09-04T11:59:00Z".into(),
                last_opened_at: None,
                degraded: None,
            })
            .collect(),
        active_context: None,
        sessions: vec![],
        statuses: vec![],
        pools: vec![],
        hosts: vec![],
        jobs: vec![],
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: String::new(),
            pid: 1,
            started_at: String::new(),
            home: String::new(),
        },
    }
}

/// A full frame for a terminal no Hub row mentions.
fn frame() -> fleet_proto::terminal::FrameUpdate {
    use fleet_proto::terminal::{CursorShape, CursorState, TerminalModes, ViewportInfo};
    fleet_proto::terminal::FrameUpdate {
        terminal: fleet_core::ids::TerminalId(1),
        seq: 1,
        cols: 1,
        rows: 1,
        full: true,
        shift: None,
        rows_changed: Vec::new(),
        cursor: CursorState {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
        },
        viewport: ViewportInfo {
            scrollback_len: 0,
            offset: 0,
            history_epoch: 0,
        },
        modes: TerminalModes::default(),
        title: None,
    }
}

#[test]
fn large_projection_is_shared_across_clock_cursor_and_terminal_only_updates() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-test", now);
    state.apply_snapshot(snapshot(10_000), now);
    let hub = HubState::default();
    let initial = projection::prepare(&state, &hub, 1_788_523_200);
    assert_eq!(initial.worktrees.len(), 10_000);
    assert!(initial.prs.is_empty());

    // Neither the clock nor the cursor is a projection input: 10 000 rows are built once.
    for tick in 0..20 {
        state.cursors.worktrees = tick;
        let cached = projection::prepare(&state, &hub, 1_788_523_201 + tick as i64);
        assert!(Rc::ptr_eq(&initial, &cached));
    }

    // Terminal output moves no snapshot revision, so it never rebuilds the Hub either.
    state.apply_frame(&frame());
    assert!(Rc::ptr_eq(
        &initial,
        &projection::prepare(&state, &hub, 1_788_523_220)
    ));

    let mut changed = snapshot(10_000);
    changed.worktrees[0].branch = "changed branch".into();
    state.apply_snapshot(changed, now);
    let updated = projection::prepare(&state, &hub, 1_788_523_221);
    assert!(!Rc::ptr_eq(&initial, &updated));
    assert!(
        updated
            .worktrees
            .iter()
            .any(|row| row.branch == "changed branch")
    );
    state.filter.query = "changed".into();
    assert_eq!(
        projection::prepare(&state, &hub, 1_788_523_222)
            .worktrees
            .len(),
        1
    );
    state.screen = Screen::Hub { tab: HubTab::Prs };
    assert!(
        projection::prepare(&state, &hub, 1_788_523_222)
            .worktrees
            .is_empty()
    );
}

#[test]
fn safety_details_prioritize_both_inspection_error_channels() {
    let mut data = inspection(false, Some(0), true);
    data.error = Some("git inspection failed".into());
    let mut slot = Inspected::ready(data);
    assert_eq!(slot.failure(), Some("git inspection failed"));
    slot.error = Some("transport failed".into());
    assert_eq!(slot.failure(), Some("transport failed"));
}
