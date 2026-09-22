use std::time::Instant;

use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationCaller, DelegationId, DelegationStatus, DeliveryState,
        GateId, GateKind, ItemId, OpenGate, Seq, ThreadId, ThreadProjection, ToolKind, TurnId,
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
        caller: DelegationCaller::Thread(caller),
        caller_turn: Some(TurnId::new()),
        caller_item: Some(ItemId::new()),
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
        usage: None,
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
fn pending_gate_waits_for_the_actionable_projection() {
    let now = Instant::now();
    let mut state = state();
    let worktree = "buk/payroll#feat"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let thread = ThreadId::new();
    let mut projection = ThreadProjection::new(thread, worktree, AgentKind::Codex);
    projection.gates.push(OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            item: None,
            tool: ToolKind::Edit,
            title: "edit README.md".to_owned(),
            payload: "README.md".to_owned(),
            rationale: None,
            options: Vec::new(),
        },
        opened_seq: Seq(1),
        blocked_since: None,
    });
    let mut snapshot = test_support::snapshot();
    snapshot
        .sessions
        .push(test_support::session_with("buk/payroll#feat", &[]));
    snapshot.agent_threads = vec![projection.summary(Seq::default())];
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: "buk/payroll#feat"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.agents.activate(
        "buk/payroll#feat"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        thread,
    );

    assert_eq!(
        state.harness_projection().snapshot.agents.threads[0].state,
        "needs_you"
    );
    assert!(
        state.harness_projection().snapshot.agents.threads[0]
            .pending_gate
            .is_none(),
        "a summary may announce attention before the projection can route an answer"
    );

    assert!(
        state.agents.deactivate(
            &"buk/payroll#feat"
                .parse()
                .unwrap_or_else(|error| panic!("{error}"))
        )
    );
    assert_eq!(
        state.harness_projection().snapshot.agents.threads[0]
            .pending_gate
            .as_deref(),
        Some("permission"),
        "an unopened background thread still exposes the gate kind carried by its summary"
    );

    state.agents.install_snapshot(projection, &[]);

    assert_eq!(
        state.harness_projection().snapshot.agents.threads[0]
            .pending_gate
            .as_deref(),
        Some("permission")
    );
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

/// The board a workflow scenario reads: a worktree board whose second column runs a card.
///
/// Built through `apply_board_view` rather than by assignment, so the marks are the ones the
/// app's own fold derives (contracts §5.2) instead of a set this test agreed with itself on.
mod workflow {
    use super::*;
    use chrono::{Duration, Utc};
    use fleet_core::{
        board::{
            Action, ActionKind, BoardView, Card, CardDraft, CardRun, ColumnAgentPrefs,
            ColumnAutomation, LiveRun, PENDING_AMBER_AFTER_SECS, PendingRun, RunOutcome,
            create_card, new_worktree_board,
        },
        model::{Context, Worktree},
    };

    const CREATED: &str = "2026-09-20T09:00:00Z";

    fn context() -> Context {
        Context {
            id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
            name: "Fleet".into(),
            owners: Vec::new(),
            created_at: CREATED.to_owned(),
        }
    }

    fn worktree() -> Worktree {
        Worktree {
            id: "acme/api#agent"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
            slug: "agent".to_owned(),
            branch: "feat/agent".to_owned(),
            base_ref: "main".to_owned(),
            path: "/tmp/agent".to_owned(),
            session: "acme/api#agent".to_owned(),
            host: None,
            created_at: CREATED.to_owned(),
            last_opened_at: None,
            degraded: None,
        }
    }

    /// Three cards in the first column of a board whose *second* column runs on arrival.
    fn view() -> BoardView {
        let mut board = new_worktree_board(&context(), &worktree(), CREATED);
        if let Some(status) = board.statuses.get_mut(1) {
            status.automation = Some(ColumnAutomation {
                on_enter: Some(Action {
                    kind: ActionKind::Prompt,
                    instructions: String::new(),
                    expect: String::new(),
                    agent: ColumnAgentPrefs::default(),
                    env: Vec::new(),
                }),
                ..ColumnAutomation::default()
            });
        }
        let mut cards = Vec::new();
        for (index, title) in ["Fix login", "Ship the board", "Write the docs"]
            .iter()
            .enumerate()
        {
            let card = create_card(
                &mut board,
                &cards,
                format!("card-{index}")
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                CardDraft {
                    title: (*title).to_owned(),
                    ..CardDraft::default()
                },
                CREATED,
            )
            .unwrap_or_else(|error| panic!("{error}"));
            cards.push(card);
        }
        BoardView {
            board,
            cards,
            live_runs: Vec::new(),
        }
    }

    fn run(card: &Card, outcome: Option<RunOutcome>) -> CardRun {
        CardRun {
            id: DelegationId::new(),
            thread_id: Some(ThreadId::new()),
            status_id: card.status_id.clone(),
            action: ActionKind::Prompt,
            provider: AgentKind::Codex,
            model: Some("gpt-5".to_owned()),
            effort: Some("high".to_owned()),
            started_at: CREATED.to_owned(),
            ended_at: outcome.is_some().then(|| "2026-09-20T09:30:00Z".to_owned()),
            outcome,
            detail: None,
            report_comment_id: None,
            files_changed: 0,
            cost_usd: None,
            tokens: None,
        }
    }

    /// The app on the Hub's board tab, pointed at that worktree board.
    fn state_with(view: BoardView) -> AppState {
        let now = Instant::now();
        let mut state = state();
        state.daemon = DaemonLink::Connected;
        let mut snapshot = test_support::snapshot();
        snapshot.active_context = Some(context().id.clone());
        snapshot.contexts = vec![context()];
        snapshot.worktrees = vec![worktree()];
        state.apply_snapshot(snapshot, now);
        state.board.scope = Some(crate::state::BoardScope::Worktree(worktree().id));
        state.apply_board_view(view);
        // `create_card` files a card under the first unstarted status, which is `Todo` — the
        // column the board opens on is `Backlog`, and a scenario reading `board.cards` has
        // moved to the cards first.
        state.board.focus.column = 1;
        state
    }

    fn marks_of(dump: &UiSnapshot, list: &str, row: usize) -> Vec<String> {
        dump.lists
            .get(list)
            .unwrap_or_else(|| panic!("{list} list"))
            .rows
            .get(row)
            .unwrap_or_else(|| panic!("{list} row {row}"))
            .marks
            .clone()
    }

    #[test]
    fn a_card_states_its_run_and_its_blockers_in_the_frozen_vocabulary() {
        let mut view = view();
        let live = run(&view.cards[0], None);
        view.live_runs.push(LiveRun {
            card_id: view.cards[0].id.clone(),
            run: live.id,
            status: DelegationStatus::Running,
            headline: None,
            started: CREATED.to_owned(),
        });
        view.cards[0].runs.push(live);
        let blocker = view.cards[0].id.clone();
        view.cards[2].blocked_by = vec![blocker];
        view.cards[2].assignee = Some("danny".to_owned());
        let state = state_with(view);

        let dump = state.harness_projection().snapshot;
        assert_eq!(
            marks_of(&dump, "board.cards", 0),
            vec!["working".to_owned()]
        );
        assert_eq!(
            marks_of(&dump, "board.cards", 2),
            vec!["blocked:1".to_owned(), "danny".to_owned()],
            "the run mark leads, the blocked count follows and the assignee comes last"
        );
        assert_eq!(
            dump.lists["board.summary"].rows[0].label, "1/1 working",
            "a count the header does not state is left out of the row"
        );
        assert_eq!(
            marks_of(&dump, "board", 1),
            vec!["action".to_owned()],
            "only a column that runs a card on arrival is marked"
        );
        assert!(marks_of(&dump, "board", 0).is_empty());
    }

    #[test]
    fn the_summary_row_is_absent_while_the_board_says_neither_count() {
        let dump = state_with(view()).harness_projection().snapshot;
        assert!(
            !dump.lists.contains_key("board.summary"),
            "a board with nothing running carries no header counts, so it carries no row"
        );
        assert!(marks_of(&dump, "board.cards", 0).is_empty());
    }

    /// `board.cards` is the column as the pane draws it, which is what makes `rows[R]` and
    /// `focused == board.column[C].card[R]` the same card.
    ///
    /// The document keeps a card where it was created and only `position` says where the column
    /// shows it, so a card `]` moved in sits last on screen while still being first in
    /// `view.cards`. A list built straight off `view.cards` reported the two in different orders
    /// and a scenario reading a row by the index `focused` had just given it got another card.
    #[test]
    fn the_card_list_is_the_column_in_the_order_the_pane_draws_it() {
        let mut view = view();
        view.cards[0].position = 30;
        let mut state = state_with(view);
        state.screen = Screen::Hub {
            tab: crate::state::HubTab::Board,
        };
        state.board.focus.row = 2;

        let dump = state.harness_projection().snapshot;
        let titles: Vec<_> = dump.lists["board.cards"]
            .rows
            .iter()
            .map(|row| row.label.clone())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Ship the board".to_owned(),
                "Write the docs".to_owned(),
                "Fix login".to_owned(),
            ],
            "the column is ordered by position, exactly as `visible_cards` orders it"
        );
        assert_eq!(dump.focused.as_deref(), Some("board.column[1].card[2]"));
        assert_eq!(
            dump.lists["board.cards"].rows[2].label, "Fix login",
            "the row `focused` names is the card the cursor is on"
        );
    }

    #[test]
    fn a_mark_only_the_clock_changed_moves_the_revision() {
        let mut view = view();
        let since = Utc::now()
            - Duration::seconds(i64::try_from(PENDING_AMBER_AFTER_SECS).unwrap_or(i64::MAX) - 5);
        view.cards[0].pending_run = Some(PendingRun {
            status_id: view.cards[0].status_id.clone(),
            since: since.to_rfc3339(),
        });
        let mut state = state_with(view);
        let before = state.harness_projection();
        assert_eq!(
            marks_of(&before.snapshot, "board.cards", 0),
            vec!["pending".to_owned()]
        );

        // The tick the app already runs for its toasts, far enough on that the wait itself is
        // worth noticing. Nothing else about the board moved.
        state.refresh_card_marks(Utc::now() + Duration::seconds(10));
        let after = state.harness_projection();
        assert_eq!(
            marks_of(&after.snapshot, "board.cards", 0),
            vec!["stalled".to_owned()]
        );
        assert_ne!(
            after.revision, before.revision,
            "a mark the clock alone changed must wake a waiting `await`"
        );
    }

    #[test]
    fn the_open_card_detail_lists_its_runs_oldest_first() {
        let mut view = view();
        let first = run(&view.cards[0], Some(RunOutcome::Succeeded));
        let second = run(&view.cards[0], None);
        view.live_runs.push(LiveRun {
            card_id: view.cards[0].id.clone(),
            run: second.id,
            status: DelegationStatus::Blocked,
            headline: None,
            started: CREATED.to_owned(),
        });
        view.cards[0].runs = vec![first, second];
        let mut state = state_with(view);
        assert!(
            !state
                .harness_projection()
                .snapshot
                .lists
                .contains_key("card.runs"),
            "the list belongs to the open dialog, not to the board behind it"
        );

        state.open_overlay(Overlay::Dialog(crate::dialogs::Dialogs::CardDetail));
        let dump = state.harness_projection().snapshot;
        let runs = &dump.lists["card.runs"];
        assert_eq!(runs.rows.len(), 2);
        assert!(runs.selected.is_none(), "the detail has no run cursor");
        assert!(
            runs.rows[0].label.starts_with("succeeded 30m"),
            "the older run speaks for itself: {}",
            runs.rows[0].label
        );
        assert_eq!(runs.rows[0].marks, vec!["done".to_owned()]);
        assert_eq!(runs.rows[0].badges, vec!["codex".to_owned()]);
        assert!(
            runs.rows[1].label.starts_with("blocked"),
            "a live run states the child's own status word: {}",
            runs.rows[1].label
        );
        assert_eq!(
            runs.rows[1].marks,
            vec!["needs you".to_owned()],
            "and the mark is the one the card's tile draws for the same run"
        );
        assert!(
            runs.rows[1]
                .label
                .contains("codex \u{b7} gpt-5 \u{b7} high"),
            "the label is the row the detail draws: {}",
            runs.rows[1].label
        );
    }

    #[test]
    fn a_card_owed_a_run_leaves_its_finished_row_speaking_for_itself() {
        let mut view = view();
        view.cards[0].runs = vec![run(&view.cards[0], Some(RunOutcome::Succeeded))];
        view.cards[0].pending_run = Some(PendingRun {
            status_id: view.cards[0].status_id.clone(),
            since: Utc::now().to_rfc3339(),
        });
        let mut state = state_with(view);
        state.open_overlay(Overlay::Dialog(crate::dialogs::Dialogs::CardDetail));

        let dump = state.harness_projection().snapshot;
        assert_eq!(
            marks_of(&dump, "board.cards", 0),
            vec!["pending".to_owned()],
            "the tile is about the run the card is owed"
        );
        assert_eq!(
            dump.lists["card.runs"].rows[0].marks,
            vec!["done".to_owned()],
            "so the run that already ended is read from its own outcome"
        );
    }

    #[test]
    fn a_card_with_no_run_lists_none() {
        let mut state = state_with(view());
        state.open_overlay(Overlay::Dialog(crate::dialogs::Dialogs::CardDetail));
        assert!(
            !state
                .harness_projection()
                .snapshot
                .lists
                .contains_key("card.runs")
        );
    }

    #[test]
    fn the_settings_columns_state_their_action_and_whether_the_board_may_carry_one() {
        let mut state = state_with(view());
        state.open_overlay(Overlay::Dialog(crate::dialogs::Dialogs::BoardSettings));
        let dump = state.harness_projection().snapshot;
        let columns = &dump.lists["settings.columns"];
        assert_eq!(
            columns.rows.len(),
            state.board().expect("board").board.statuses.len()
        );
        assert_eq!(columns.rows[1].marks, vec!["action".to_owned()]);
        assert!(columns.rows[0].marks.is_empty());
        assert_eq!(
            columns.rows[1].label,
            state.board().expect("board").board.statuses[1].name
        );
    }

    #[test]
    fn a_board_that_may_not_carry_automation_marks_every_column_disabled() {
        let mut view = view();
        view.board.worktree_id = None;
        let mut state = state_with(view);
        state.board.view = None;
        // A context board: the same columns, and no checkout for any of them to run in.
        let mut context_view = self::view();
        context_view.board.worktree_id = None;
        state.board.scope = Some(crate::state::BoardScope::Context(context().id));
        state.apply_board_view(context_view);
        state.open_overlay(Overlay::Dialog(crate::dialogs::Dialogs::BoardSettings));
        let dump = state.harness_projection().snapshot;
        let columns = &dump.lists["settings.columns"];
        assert_eq!(columns.rows[0].marks, vec!["disabled".to_owned()]);
        assert_eq!(
            columns.rows[1].marks,
            vec!["action".to_owned(), "disabled".to_owned()]
        );
    }
}
