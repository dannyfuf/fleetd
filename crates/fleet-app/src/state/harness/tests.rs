use std::time::Instant;

use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationId, DelegationStatus, DeliveryState, ItemId, Seq,
        ThreadId, ThreadProjection, TurnId,
    },
    ids::TerminalId,
    model::Worktree,
    sessions::{SessionKind, Terminal as SessionTerminal, TerminalKind, TerminalStatus},
};
use fleet_proto::{
    job::{JobKind, JobRecord, JobStatus},
    terminal::{Cell, CellAttrs, CellWidth, Color, RowUpdate},
};
use fleet_ui_kit::{Icon, Toast, ToastDuration};

use super::*;
use crate::state::{
    AppState, DaemonLink, DaemonLossReason, Overlay, Screen, dwell_for, test_support,
};

pub(super) fn state() -> AppState {
    AppState::new("/tmp/fleet-harness", Instant::now())
}

pub(super) fn job(id: &str, status: JobStatus) -> JobRecord {
    JobRecord {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::Prune,
        target: "acme/api".to_owned(),
        title: format!("prune {id}"),
        status,
        progress: None,
        log_path: "/tmp/job.log".to_owned(),
        started_at: "2026-09-11T09:00:00Z".to_owned(),
        finished_at: None,
        cancellable: true,
        retryable: false,
    }
}

pub(super) fn cell(text: &str, width: CellWidth) -> Cell {
    Cell {
        text: text.into(),
        fg: Color::Default,
        bg: Color::Default,
        underline_color: None,
        attrs: CellAttrs::empty(),
        width,
    }
}

#[test]
fn serialized_shape_is_pinned() {
    let snapshot = state().harness_snapshot();
    assert_eq!(
        serde_json::to_value(&snapshot).expect("serialize snapshot"),
        serde_json::json!({
            "version":1,"screen":"Hub","mode":"Normal","hub_pane":"List","hub_tab":"Worktrees","overlay":null,
            "key_contexts":["Hub","Worktrees"],"focused":"worktrees.row[0]","lists":{},"dialog":null,"toasts":[],
            "sticky_error":null,"jobs":[],
            "agents":{"popup":null,"threads":[],"delegations":[],"decision":null},"terminal":null,"targets":{},
            "daemon":{"link":"starting","attempt":0,"dismissed":false,"restarted":false},
            "idle":{"idle":false,"in_flight_requests":0,"running_jobs":0,"pending_frame":false,"live_toast_timers":0,"armed_debounces":0,"settling_mutations":0,"link_opening":true},
            "window":{"bounds":{"x":0.0,"y":0.0,"w":0.0,"h":0.0},"scale_factor":0.0,"title":"","frame":0}
        })
    );
}

#[test]
fn populated_state_projects_every_collection() {
    let now = Instant::now();
    let mut state = state();
    state.daemon = DaemonLink::Connected;
    let mut snapshot = test_support::snapshot();
    let worktree: fleet_core::ids::WorktreeId = "acme/api#login"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    snapshot.worktrees.push(Worktree {
        id: worktree.clone(),
        repo_id: "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
        slug: "login".to_owned(),
        branch: "feat/login".to_owned(),
        base_ref: "main".to_owned(),
        path: "/tmp/login".to_owned(),
        session: "acme/api#login".to_owned(),
        host: None,
        created_at: "2026-09-11T09:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    snapshot.jobs.push(job("job-1", JobStatus::Running));
    state.apply_snapshot(snapshot, now);
    state.displayed_hub.worktrees = vec![crate::presentation::DisplayedWorktree {
        id: worktree,
        repo: "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
    }];
    state.toast(
        Toast::new("cloned").icon(Icon::Info),
        now,
        dwell_for(ToastDuration::Normal),
    );

    let dump = state.harness_projection().snapshot;
    let worktrees = dump.lists.get("worktrees").expect("worktrees list");
    assert_eq!(
        worktrees.selected.as_ref().map(|row| row.label.as_str()),
        Some("login")
    );
    assert_eq!(worktrees.rows[0].badges, vec!["feat/login".to_owned()]);
    assert_eq!(dump.jobs.len(), 1);
    assert_eq!(dump.jobs[0].status, "running");
    assert_eq!(dump.toasts[0].level, "info");
    assert_eq!(dump.toasts[0].text, "cloned");
    assert!(dump.lists.contains_key("jobs"));
}

#[test]
fn a_running_job_keeps_idle_false_until_it_finishes() {
    let now = Instant::now();
    let mut state = state();
    state.daemon = DaemonLink::Connected;
    let mut snapshot = test_support::snapshot();
    snapshot.jobs.push(job("job-1", JobStatus::Running));
    state.apply_snapshot(snapshot, now);

    let busy = state.harness_projection();
    assert_eq!(busy.snapshot.idle.running_jobs, 1);
    assert!(!busy.snapshot.idle.idle, "a running job is not idle");

    state.apply_job(job("job-1", JobStatus::Succeeded), now);
    let settled = state.harness_projection();
    assert_eq!(settled.snapshot.idle.running_jobs, 0);
    assert!(settled.snapshot.idle.idle, "the completion flips idle");
    assert!(
        settled.revision > busy.revision,
        "a state change moves the revision"
    );
}

#[test]
fn every_pending_source_alone_defeats_idle() {
    let now = Instant::now();
    let mut state = state();
    state.daemon = DaemonLink::Connected;
    let in_flight = Arc::new(AtomicU32::new(1));
    state.harness.attach_bridge(
        Arc::clone(&in_flight),
        Arc::new(SettleCounter::default()),
        IdleWake::new(|| {}),
    );
    assert!(!state.harness_projection().snapshot.idle.idle);
    in_flight.fetch_sub(1, Ordering::AcqRel);
    assert!(state.harness_projection().snapshot.idle.idle);

    state.harness.set_pending_frame(true);
    assert!(!state.harness_projection().snapshot.idle.idle);
    state.harness.set_pending_frame(false);

    let debounce = state.harness.arm_debounce();
    let armed = state.harness_projection().snapshot;
    assert_eq!(armed.idle.armed_debounces, 1);
    assert!(!armed.idle.idle);
    drop(debounce);
    assert!(
        state.harness_projection().snapshot.idle.idle,
        "dropping the guard disarms the window a superseded task never got to release"
    );

    state.toast(
        Toast::new("copied").icon(Icon::Info),
        now,
        dwell_for(ToastDuration::Short),
    );
    let toasting = state.harness_projection().snapshot;
    assert_eq!(toasting.idle.live_toast_timers, 1);
    assert!(!toasting.idle.idle, "a live toast timer is pending work");
}

#[test]
fn settle_counter_expires_only_legacy_unsettled_claims() {
    let settle = SettleCounter::default();
    let expired = settle.begin(None);
    assert_eq!(settle.pending(), 1);
    assert!(settle.expire(expired));
    assert_eq!(settle.pending(), 0);
    assert!(!settle.expire(expired), "one mutation is released once");

    let settled = settle.begin(Some(4));
    settle.applied(Some(4));
    assert_eq!(settle.pending(), 0);
    assert!(
        !settle.expire(settled),
        "a snapshot invalidates its mutation's grace expiry"
    );

    let next = settle.begin(Some(5));
    assert_ne!(next, settled);
    assert!(!settle.expire(next));
    assert_eq!(settle.pending(), 1);
    settle.applied(Some(5));
    assert_eq!(settle.pending(), 0);
    assert_eq!(MUTATION_SETTLE_GRACE, std::time::Duration::from_millis(250));
}

#[test]
fn settling_mutations_and_an_opening_link_each_defeat_idle() {
    assert!(!IdleSnapshot::new(0, 0, false, 0, 0, 1, false).idle);
    assert!(!IdleSnapshot::new(0, 0, false, 0, 0, 0, true).idle);
    assert!(IdleSnapshot::new(0, 0, false, 0, 0, 0, false).idle);
}

#[test]
fn dropping_an_armed_debounce_wakes_after_decrementing() {
    let wakes = Arc::new(AtomicU32::new(0));
    let callback_wakes = Arc::clone(&wakes);
    let mut harness = HarnessState::default();
    harness.attach_bridge(
        Arc::new(AtomicU32::new(0)),
        Arc::new(SettleCounter::default()),
        IdleWake::new(move || {
            callback_wakes.fetch_add(1, Ordering::AcqRel);
        }),
    );

    let debounce = harness.arm_debounce();
    assert_eq!(harness.armed_debounces(), 1);
    drop(debounce);
    assert_eq!(harness.armed_debounces(), 0);
    assert_eq!(wakes.load(Ordering::Acquire), 1);
}

#[test]
fn the_memo_is_lazy_and_does_not_rebuild_without_a_change() {
    let mut state = state();
    assert!(
        !state.harness_cache_allocated(),
        "an app with no harness never allocates the cache"
    );
    state.overlay = Some(Overlay::Palette);
    assert!(!state.harness_cache_allocated());

    let first = state.harness_projection();
    assert_eq!(state.harness_builds(), 1);
    let second = state.harness_projection();
    assert_eq!(
        state.harness_builds(),
        1,
        "two dumps with no state change must not rebuild"
    );
    assert_eq!(first.revision, second.revision);
    assert_eq!(first.snapshot, second.snapshot);

    state.overlay = None;
    let third = state.harness_projection();
    assert_eq!(state.harness_builds(), 2);
    assert_eq!(
        third.revision,
        first.revision + 1,
        "a visible change moves the revision exactly once"
    );
    assert_eq!(third.snapshot.overlay, None);
}

#[test]
fn an_invisible_input_change_rebuilds_but_holds_the_revision() {
    let mut state = state();
    let first = state.harness_projection();
    // A repaint moves `snapshot_revision` without changing anything the harness can see.
    state.bump_snapshot_revision();
    let second = state.harness_projection();
    assert_eq!(state.harness_builds(), 2, "the key moved, so it rebuilt");
    assert_eq!(
        first.revision, second.revision,
        "identical content keeps the revision, so `await` never wakes for nothing"
    );
}

#[test]
fn overlays_and_dialogs_name_themselves_in_the_documented_vocabulary() {
    let mut state = state();
    state.open_overlay(Overlay::Dialog(crate::dialogs::Dialogs::CreateWorktree));
    let dump = state.harness_projection().snapshot;
    assert_eq!(dump.overlay.as_deref(), Some("Dialog"));
    assert_eq!(dump.mode, "Dialog");
    assert_eq!(dump.key_contexts, vec!["Dialog", "Create"]);
    assert_eq!(
        dump.dialog.as_ref().map(|dialog| dialog.name.as_str()),
        Some("Create")
    );
    assert_eq!(dump.focused.as_deref(), Some("dialog"));
}

#[test]
fn the_terminal_is_text_with_its_columns_intact() {
    let now = Instant::now();
    let mut state = state();
    let mut snapshot = test_support::snapshot();
    let session_id = "acme/api#login";
    let mut session = test_support::session_with(session_id, &[1]);
    session.kind = SessionKind::Worktree(
        "acme/api#login"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    );
    session.active_terminal = Some(TerminalId(1));
    snapshot.sessions.push(session);
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: session_id.parse().unwrap_or_else(|error| panic!("{error}")),
    };

    let mut frame = test_support::frame(
        1,
        true,
        vec![RowUpdate {
            index: 0,
            wrapped: false,
            cells: vec![
                cell("漢", CellWidth::Wide),
                cell("", CellWidth::Spacer),
                cell("", CellWidth::Narrow),
                cell("x", CellWidth::Narrow),
                cell(" ", CellWidth::Narrow),
            ],
        }],
    );
    frame.cols = 5;
    assert!(
        state.apply_frame(&frame),
        "the full frame primes the mirror"
    );

    let dump = state.harness_projection().snapshot;
    let terminal = dump.terminal.as_ref().expect("a visible terminal");
    assert_eq!(
        terminal.rows,
        vec!["漢 x".to_owned()],
        "the spacer is dropped, the blank cell survives, the padding goes"
    );
    assert_eq!(terminal.text, "漢 x");
    assert_eq!(terminal.cursor.shape, "block");
    assert_eq!(terminal.viewport.rows, 2);
}

#[test]
fn the_terminal_field_is_absent_without_a_visible_terminal() {
    assert!(state().harness_projection().snapshot.terminal.is_none());
}

#[test]
fn terminal_output_moves_the_revision() {
    let now = Instant::now();
    let mut state = state();
    let mut snapshot = test_support::snapshot();
    let session_id = "acme/api#login";
    let mut session = test_support::session_with(session_id, &[1]);
    session.active_terminal = Some(TerminalId(1));
    snapshot.sessions.push(session);
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: session_id.parse().unwrap_or_else(|error| panic!("{error}")),
    };
    assert!(state.apply_frame(&test_support::frame(
        1,
        true,
        vec![RowUpdate {
            index: 0,
            wrapped: false,
            cells: vec![cell("h", CellWidth::Narrow), cell("i", CellWidth::Narrow)],
        }],
    )));
    let before = state.harness_projection();

    assert!(state.apply_frame(&test_support::frame(
        2,
        false,
        vec![RowUpdate {
            index: 0,
            wrapped: false,
            cells: vec![cell("o", CellWidth::Narrow), cell("k", CellWidth::Narrow)],
        }],
    )));
    let after = state.harness_projection();
    assert_eq!(
        after.snapshot.terminal.as_ref().map(|t| t.text.as_str()),
        Some("ok")
    );
    assert!(after.revision > before.revision);
}

#[test]
fn tabs_list_the_session_terminals() {
    let now = Instant::now();
    let mut state = state();
    let mut snapshot = test_support::snapshot();
    let session_id = "acme/api#login";
    let mut session = test_support::session_with(session_id, &[1]);
    session.active_terminal = Some(TerminalId(1));
    session.terminals.push(SessionTerminal {
        id: TerminalId(2),
        name: "logs".to_owned(),
        command: "sh".to_owned(),
        cwd: "/tmp".to_owned(),
        shell_pid: None,
        foreground_command: None,
        status: TerminalStatus::Running,
        title: None,
        keep_alive: Vec::new(),
        has_unseen_output: true,
        agent_attention: None,
        kind: TerminalKind::Native,
    });
    snapshot.sessions.push(session);
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: session_id.parse().unwrap_or_else(|error| panic!("{error}")),
    };

    let dump = state.harness_projection().snapshot;
    let tabs = dump.lists.get("tabs").expect("a tab strip");
    assert_eq!(tabs.rows.len(), 2);
    assert_eq!(tabs.rows[1].badges, vec!["native".to_owned()]);
    assert_eq!(tabs.rows[1].marks, vec!["unseen".to_owned()]);
    assert_eq!(tabs.selected.as_ref().map(|row| row.id.as_str()), Some("1"));
    assert_eq!(dump.focused.as_deref(), Some("tabs.tab[0]"));
}

#[test]
fn native_children_and_delegations_are_additive_snapshot_fields() {
    let now = Instant::now();
    let mut state = state();
    let worktree: fleet_core::ids::WorktreeId = "buk/payroll#feat"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let caller = ThreadId::new();
    let child = ThreadId::new();
    let mut caller_projection = ThreadProjection::new(caller, worktree.clone(), AgentKind::Claude);
    caller_projection.title = "coordinate release".to_owned();
    let mut child_projection = ThreadProjection::new(child, worktree, AgentKind::Codex);
    child_projection.parent = Some(caller);
    child_projection.title = "inspect reducer".to_owned();
    let mut snapshot = test_support::snapshot();
    snapshot
        .sessions
        .push(test_support::session_with("buk/payroll#feat", &[]));
    snapshot.agent_threads = vec![
        caller_projection.summary(Seq::default()),
        child_projection.summary(Seq::default()),
    ];
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: "buk/payroll#feat"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    assert!(state.agents.attach(child));
    let record = Delegation {
        id: DelegationId::new(),
        caller,
        caller_turn: TurnId::new(),
        caller_item: ItemId::new(),
        child,
        provider: AgentKind::Codex,
        depth: 1,
        brief: "inspect reducer".to_owned(),
        expectation: "report the invariant".to_owned(),
        eager: false,
        status: DelegationStatus::Running,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: chrono::DateTime::UNIX_EPOCH,
        finished: None,
        headline: Some("reading state transitions".to_owned()),
    };
    state.agents.seed_delegations(vec![record.clone()]);

    let before = state.harness_projection();
    let child_snapshot = before
        .snapshot
        .agents
        .threads
        .iter()
        .find(|thread| thread.id == child.to_string())
        .expect("child snapshot");
    assert_eq!(
        child_snapshot.parent.as_deref(),
        Some(caller.to_string().as_str())
    );
    assert!(child_snapshot.attached);
    assert_eq!(before.snapshot.agents.delegations.len(), 1);
    assert_eq!(before.snapshot.agents.delegations[0].status, "working");
    assert_eq!(before.snapshot.agents.delegations[0].delivery, "pending");
    assert_eq!(
        before
            .snapshot
            .lists
            .get("tabs")
            .and_then(|tabs| tabs.rows.iter().find(|row| row.id == child.to_string()))
            .map(|row| row.badges.clone()),
        Some(vec!["codex".to_owned(), "child".to_owned()])
    );

    assert!(state.agents.detach(child));
    let detached = state.harness_projection();
    assert!(detached.revision > before.revision);
    assert!(
        !detached
            .snapshot
            .agents
            .threads
            .iter()
            .find(|thread| thread.id == child.to_string())
            .expect("child snapshot")
            .attached
    );
    assert!(
        detached
            .snapshot
            .lists
            .get("tabs")
            .is_some_and(|tabs| tabs.rows.iter().all(|row| row.id != child.to_string()))
    );

    let mut finished = record;
    finished.status = DelegationStatus::Succeeded;
    state.agents.apply_delegation(finished);
    let completed = state.harness_projection();
    assert!(completed.revision > detached.revision);
    assert_eq!(completed.snapshot.agents.delegations[0].status, "done");
}

#[test]
fn recorded_targets_and_window_metrics_reach_the_snapshot() {
    let mut state = state();
    state.harness.set_window(
        BoundsSnapshot {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: 900.0,
        },
        2.0,
        "fleet-harness",
        7,
    );
    state
        .harness
        .set_targets(std::collections::BTreeMap::from([(
            "worktrees.row[0]".to_owned(),
            TargetSnapshot {
                x: 12.0,
                y: 48.0,
                w: 300.0,
                h: 24.0,
                frame: 7,
            },
        )]));
    let dump = state.harness_projection().snapshot;
    assert_eq!(dump.window.frame, 7);
    assert_eq!(dump.window.title, "fleet-harness");
    assert_eq!(
        dump.targets.get("worktrees.row[0]").map(|target| target.h),
        Some(24.0)
    );
}

#[test]
fn the_summary_line_names_the_moment() {
    let mut state = state();
    state.open_overlay(Overlay::Palette);
    let summary = state.harness_projection().snapshot.summary();
    assert!(summary.contains("mode=Palette"), "{summary}");
    assert!(summary.contains("focus=palette.input"), "{summary}");
}

#[test]
fn the_daemon_link_is_reported_where_the_key_contexts_cannot_show_it() {
    let now = Instant::now();
    let mut state = state();
    // The chain carries no daemon surface here — `context_chain` appends `Daemon > Banner`
    // only behind a base surface it recognises — which is the case the `daemon` field exists
    // for.
    state.daemon = DaemonLink::Lost {
        attempt: 3,
        dismissed: true,
        reason: DaemonLossReason::ConnectionLost,
    };
    let snapshot = state.harness_projection().snapshot;
    assert!(
        !snapshot.key_contexts.iter().any(|word| word == "Daemon"),
        "the chain shows no daemon surface: {:?}",
        snapshot.key_contexts
    );
    assert_eq!(
        snapshot.daemon,
        DaemonSnapshot {
            link: "lost",
            attempt: 3,
            dismissed: true,
            restarted: false,
        }
    );

    // The link is a projection input, so moving it must move the revision a waiting `await`
    // wakes on. A key that missed it would leave the memo — and the scenario — where it was.
    let before = state.harness_projection().revision;
    state.daemon = DaemonLink::Reconnected {
        restarted: true,
        reattached: 2,
        since: now,
    };
    assert_eq!(
        state.daemon,
        DaemonLink::Reconnected {
            restarted: true,
            reattached: 2,
            since: now,
        }
    );
    let after = state.harness_projection();
    assert_ne!(after.revision, before);
    assert_eq!(
        after.snapshot.daemon,
        DaemonSnapshot {
            link: "reconnected",
            attempt: 0,
            dismissed: false,
            restarted: true,
        }
    );
}
