use super::lifecycle::invalidate_history_epoch;
use crate::{
    state::MirrorGrid,
    terminal::{AbsoluteCellPoint, AbsoluteCellSelection, cached_grid_row, grid_size},
};
use gpui::{AppContext, Bounds, Keystroke};
use std::{cell::RefCell, collections::BTreeMap};

use fleet_core::{
    ids::JobId,
    sessions::{TerminalKind, TerminalStatus},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    job::JobKind,
};
use gpui::Modifiers as GpuiModifiers;

use super::*;

#[gpui::test]
fn rejected_workspace_input_is_visible(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/fleet-workspace-input", Instant::now()));

    cx.update(|cx| surface::report_input_delivery(false, &state, cx));

    state.read_with(cx, |app, _| {
        assert!(
            app.toasts
                .iter()
                .any(|live| { live.toast.text.as_ref() == "input dropped while attaching" })
        );
    });
}

fn keystroke(key: &str, key_char: Option<&str>, mods: GpuiModifiers) -> Keystroke {
    Keystroke {
        modifiers: mods,
        key: key.to_owned(),
        key_char: key_char.map(str::to_owned),
    }
}

fn job(target: &str, status: JobStatus) -> JobRecord {
    JobRecord {
        id: JobId::try_from("job-1").unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::Prune,
        target: target.to_owned(),
        title: "prune".to_owned(),
        status,
        progress: None,
        log_path: "/tmp/job.log".to_owned(),
        started_at: "2026-09-04T00:00:00Z".to_owned(),
        finished_at: None,
        cancellable: false,
        retryable: false,
    }
}

fn terminal(id: u64, kind: TerminalKind) -> Terminal {
    Terminal {
        id: TerminalId(id),
        name: format!("terminal-{id}"),
        command: "shell".to_owned(),
        cwd: "/tmp".to_owned(),
        shell_pid: None,
        foreground_command: None,
        status: TerminalStatus::Running,
        title: None,
        keep_alive: Vec::new(),
        has_unseen_output: false,
        kind,
    }
}

fn session(id: &str, terminal: Terminal) -> Session {
    Session {
        id: SessionId::try_from(id).unwrap_or_else(|error| panic!("{error}")),
        kind: SessionKind::Agent(fleet_core::config::Agent::Claude),
        cwd: "/tmp".to_owned(),
        active_terminal: Some(terminal.id),
        terminals: vec![terminal],
        slept_at: None,
        kept_terminals: Vec::new(),
    }
}

fn app_with_session(session: Session) -> AppState {
    let mut app = AppState::new("/tmp/fleet-workspace-test", Instant::now());
    app.screen = Screen::Workspace {
        session: session.id.clone(),
    };
    app.snapshot = Some(fleet_proto::snapshot::Snapshot {
        generated_at: "2026-09-04T12:00:00Z".to_owned(),
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: None,
        sessions: vec![session],
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "0.1.0".to_owned(),
            pid: 42,
            started_at: "2026-09-04T09:00:00Z".to_owned(),
            home: "/tmp/fleet".to_owned(),
        },
    });
    app
}

/// A worktree session with one PTY tab plus one native agent thread whose tab is selected.
///
/// This is the shape S1 was reported in: an agent tab is client state laid over the same strip,
/// so `session.active_terminal` still names the PTY the user came from.
fn app_showing_an_agent_tab() -> (AppState, fleet_core::agents::ThreadId) {
    let worktree: WorktreeId = "buk/payroll#feat"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let mut record = session("payroll/feat", terminal(1, TerminalKind::Pty));
    record.kind = SessionKind::Worktree(worktree.clone());
    let mut app = app_with_session(record.clone());
    let projection = fleet_core::agents::ThreadProjection::new(
        fleet_core::agents::ThreadId::new(),
        worktree.clone(),
        fleet_core::agents::AgentKind::Claude,
    );
    let thread = projection.thread;
    let mut snapshot = app
        .snapshot
        .clone()
        .unwrap_or_else(|| panic!("app_with_session installs a snapshot"));
    snapshot.agent_threads = vec![projection.summary(fleet_core::agents::Seq::default())];
    app.apply_snapshot(snapshot, Instant::now());
    app.screen = Screen::Workspace {
        session: record.id.clone(),
    };
    app.agents.activate(worktree, thread);
    (app, thread)
}

/// S1: characters typed into an agent tab's composer were forwarded to a background PTY, where
/// they edited whatever program was running in tab 1. The composer never saw them, because the
/// Workspace's key-down listener stopped propagation before gpui reached the input handler.
#[test]
fn an_agent_tab_leaves_no_pty_listening_for_the_keys_typed_into_its_composer() {
    let (app, _thread) = app_showing_an_agent_tab();

    assert!(app.active_agent_thread().is_some());
    assert!(
        app.active_tab_is_fleet_drawn(),
        "an agent tab is drawn by Fleet even though the session still names a PTY"
    );
    assert_eq!(
        app.active_session()
            .and_then(|session| session.active_terminal),
        Some(TerminalId(1)),
        "the reported bug: the daemon-side selection still points at the PTY"
    );
    assert_eq!(
        terminal_input_target_of(&app),
        None,
        "nothing may be typed into that PTY while the composer is on screen"
    );
    assert!(!workspace_terminal_is_live_owner(&app, Some(TerminalId(1))));
    assert_eq!(
        app.resting_terminal_mode(),
        TerminalMode::Native,
        "a Fleet-drawn tab rests in Native, which is the second gate `forward_terminal_key` reads"
    );
}

/// The same guard must not deafen a real terminal tab: leaving the agent tab hands the keys back.
#[test]
fn leaving_the_agent_tab_gives_the_pty_its_keys_back() {
    let (mut app, _thread) = app_showing_an_agent_tab();
    let worktree: WorktreeId = "buk/payroll#feat"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(app.agents.deactivate(&worktree));
    app.sync_terminal_mode();

    assert_eq!(terminal_input_target_of(&app), Some((TerminalId(1), false)));
    assert!(workspace_terminal_is_live_owner(&app, Some(TerminalId(1))));
    assert_eq!(app.resting_terminal_mode(), TerminalMode::Terminal);
}

/// S2: `^s x` acknowledged, deselected the tab — and the very next summary broadcast drew it
/// again, so the tab "survived" and the selection appeared to jump to terminal 1.
#[test]
fn a_closed_agent_tab_leaves_the_strip_and_stays_gone() {
    let (mut app, thread) = app_showing_an_agent_tab();
    let worktree: WorktreeId = "buk/payroll#feat"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(app.agents.of_worktree(&worktree).len(), 1);

    // What `close_agent_tab` does, minus the daemon round trip.
    assert!(app.agents.deactivate(&worktree));
    assert!(app.agents.close(thread));

    assert!(app.agents.of_worktree(&worktree).is_empty());
    assert!(app.active_agent_thread().is_none());

    // §6 keeps the thread browsable, so the daemon goes on listing it; the strip must not.
    let snapshot = app
        .snapshot
        .clone()
        .unwrap_or_else(|| panic!("app_with_session installs a snapshot"));
    app.apply_snapshot(snapshot, Instant::now());
    assert!(
        app.agents.of_worktree(&worktree).is_empty(),
        "a redrawn summary must not resurrect a tab the user closed"
    );
    assert!(app.agents.is_closed(thread));
}

fn encoded(key: &str, key_char: Option<&str>, mods: GpuiModifiers) -> KeyEvent {
    key_event(&keystroke(key, key_char, mods), false)
        .unwrap_or_else(|| panic!("`{key}` must encode"))
}

fn local_with(configure: impl FnOnce(&mut Local)) -> Local {
    let mut local = Local::default();
    configure(&mut local);
    local
}

#[test]
fn watch_split_scales_and_the_terminal_uses_its_reduced_measured_area() {
    assert_eq!(watch_width(800.0), 360.0);
    assert_eq!(watch_width(1200.0), 480.0);
    assert_eq!(watch_width(2000.0), 640.0);
    let cell = gpui::size(px(10.0), px(20.0));
    let mut local = local_with(|local| {
        local.area = Bounds::new(
            gpui::point(px(0.0), px(0.0)),
            gpui::size(px(1200.0), px(600.0)),
        );
    });
    let full = local.size_for(TerminalId(1), cell);
    local.area.size.width -= px(watch_width(1200.0) + 1.0);
    let split = local.size_for(TerminalId(1), cell);
    assert!(split.0 < full.0);
    assert_eq!(split.1, full.1);
    assert_eq!(
        split,
        grid_size(
            local.area.size,
            cell,
            fleet_ui_kit::theme::Spacing::default().sm
        )
    );
}

#[test]
fn a_plain_character_carries_its_composed_text() {
    let event = encoded("a", Some("a"), GpuiModifiers::default());
    assert_eq!(event.key, Key::Char('a'));
    assert_eq!(event.text.as_deref(), Some("a"));
    assert_eq!(event.mods, Modifiers::empty());
    assert_eq!(event.action, KeyAction::Press);
}

#[test]
fn the_semantic_key_stays_unshifted_while_the_text_is_shifted() {
    let mods = GpuiModifiers {
        shift: true,
        ..GpuiModifiers::default()
    };
    let event = encoded("a", Some("A"), mods);
    assert_eq!(event.key, Key::Char('a'));
    assert_eq!(event.text.as_deref(), Some("A"));
    assert!(event.mods.contains(Modifiers::SHIFT));
}

#[test]
fn control_keys_carry_no_text() {
    let mods = GpuiModifiers {
        control: true,
        ..GpuiModifiers::default()
    };
    let event = encoded("c", None, mods);
    assert_eq!(event.key, Key::Char('c'));
    assert_eq!(event.text, None);
    assert!(event.mods.contains(Modifiers::CTRL));
}

#[test]
fn named_keys_never_send_text() {
    for (name, expected) in [
        ("enter", Key::Enter),
        ("escape", Key::Escape),
        ("backspace", Key::Backspace),
        ("tab", Key::Tab),
        ("up", Key::Up),
        ("pagedown", Key::PageDown),
        ("f5", Key::F5),
    ] {
        let event = encoded(name, Some("\r"), GpuiModifiers::default());
        assert_eq!(event.key, expected, "{name}");
        assert_eq!(event.text, None, "{name}");
    }
}

#[test]
fn nvim_mode_keys_survive_the_app_translation() {
    let plain = GpuiModifiers::default();
    assert_eq!(encoded("escape", None, plain).key, Key::Escape);
    assert_eq!(encoded("i", Some("i"), plain).text.as_deref(), Some("i"));
    assert_eq!(encoded("v", Some("v"), plain).text.as_deref(), Some("v"));
    assert_eq!(encoded("up", None, plain).key, Key::Up);
    assert_eq!(encoded("down", None, plain).key, Key::Down);

    let shift = GpuiModifiers {
        shift: true,
        ..GpuiModifiers::default()
    };
    let colon = encoded(";", Some(":"), shift);
    assert_eq!(colon.key, Key::Char(';'));
    assert_eq!(colon.text.as_deref(), Some(":"));
    let shift_up = encoded("up", None, shift);
    assert_eq!(shift_up.key, Key::Up);
    assert!(shift_up.mods.contains(Modifiers::SHIFT));

    let control = GpuiModifiers {
        control: true,
        ..GpuiModifiers::default()
    };
    let ctrl_bracket = encoded("[", None, control);
    assert_eq!(ctrl_bracket.key, Key::Char('['));
    assert_eq!(ctrl_bracket.text, None);
    assert!(ctrl_bracket.mods.contains(Modifiers::CTRL));
    for key in ["c", "w"] {
        let event = encoded(key, None, control);
        assert_eq!(event.key, Key::Char(key.chars().next().unwrap_or_default()));
        assert_eq!(event.text, None);
        assert!(event.mods.contains(Modifiers::CTRL));
    }

    let alt = GpuiModifiers {
        alt: true,
        ..GpuiModifiers::default()
    };
    let alt_x = encoded("x", Some("x"), alt);
    assert_eq!(alt_x.key, Key::Char('x'));
    assert_eq!(alt_x.text.as_deref(), Some("x"));
    assert!(alt_x.mods.contains(Modifiers::ALT));
}

#[test]
fn space_is_a_character_not_a_named_key() {
    assert_eq!(
        encoded("space", Some(" "), GpuiModifiers::default()).key,
        Key::Char(' ')
    );
}

#[test]
fn a_held_key_is_a_repeat() {
    let event = key_event(&keystroke("a", Some("a"), GpuiModifiers::default()), true)
        .unwrap_or_else(|| panic!("printable keys must encode"));
    assert_eq!(event.action, KeyAction::Repeat);
}

#[test]
fn unmodelled_keys_are_dropped_rather_than_typed() {
    for name in ["f13", "back", "", "shift"] {
        assert!(
            key_event(&keystroke(name, None, GpuiModifiers::default()), false).is_none(),
            "`{name}` must not reach the pty"
        );
    }
}

#[test]
fn every_modifier_survives_the_translation() {
    let mods = GpuiModifiers {
        control: true,
        alt: true,
        shift: true,
        platform: true,
        function: true,
    };
    let event = encoded("a", None, mods);
    assert!(event.mods.contains(Modifiers::CTRL));
    assert!(event.mods.contains(Modifiers::ALT));
    assert!(event.mods.contains(Modifiers::SHIFT));
    assert!(event.mods.contains(Modifiers::SUPER));
}

#[test]
fn only_this_sessions_jobs_reach_the_header_chip() {
    let jobs = vec![
        job("acme/api#feature", JobStatus::Running),
        job(
            "acme/api#feature",
            JobStatus::Failed {
                error: "boom".to_owned(),
            },
        ),
        job("acme/api#other", JobStatus::Running),
        job("acme/api#feature", JobStatus::Succeeded),
    ];
    let targets = vec!["acme/api#feature".to_owned()];
    assert_eq!(job_counts(&jobs, &targets), (1, 1));
    assert_eq!(job_counts(&jobs, &[]), (0, 0));
}

#[test]
fn a_queued_job_already_counts_as_running() {
    let jobs = vec![job("t", JobStatus::Queued)];
    assert_eq!(job_counts(&jobs, &["t".to_owned()]), (1, 0));
}

#[test]
fn uuid_target_and_cancelling_job_count_as_active() {
    let jobs = vec![job(
        "acme/api#feature:123e4567-e89b-12d3-a456-426614174000",
        JobStatus::Cancelling,
    )];
    assert_eq!(job_counts(&jobs, &["acme/api#feature".to_owned()]), (1, 0));
}

#[test]
fn missing_status_and_host_remain_unknown() {
    assert_eq!(
        workspace_status(None, false, false, None),
        StatusKind::Unknown
    );
    assert_eq!(
        workspace_status(
            Some((SessionState::Attached, AgentActivity::Unknown)),
            false,
            false,
            Some(HostReachability::Unknown),
        ),
        StatusKind::Unknown
    );
    assert!(!HostReachability::Unknown.is_reachable());
    assert_eq!(
        workspace_status(
            Some((SessionState::Attached, AgentActivity::Unknown)),
            false,
            false,
            Some(HostReachability::Unreachable),
        ),
        StatusKind::HostUnreachable
    );
}

#[test]
fn late_new_terminal_does_not_cross_sessions() {
    let origin = SessionId::try_from("session/one").unwrap_or_else(|error| panic!("{error}"));
    let other = SessionId::try_from("session/two").unwrap_or_else(|error| panic!("{error}"));
    let response = ResponseBody::Terminal(terminal(7, TerminalKind::Pty));
    assert_eq!(
        new_terminal_reply_target(&origin, Some(&other), &response),
        None
    );
    assert_eq!(new_terminal_reply_target(&origin, None, &response), None);
    assert_eq!(
        new_terminal_reply_target(&origin, Some(&origin), &response),
        Some(TerminalId(7))
    );
}

#[test]
fn native_tabs_reject_pty_paste_and_restart() {
    let native = session("native/session", terminal(7, TerminalKind::Native));
    let pty = session("pty/session", terminal(8, TerminalKind::Pty));
    let native_app = app_with_session(native.clone());
    assert_eq!(terminal_input_target_of(&native_app), None);
    assert!(MutationRequest::restart(&native).is_none());

    let mut pty_app = app_with_session(pty.clone());
    let mut grid = MirrorGrid::new(80, 24);
    grid.primed = true;
    pty_app.grids.insert(TerminalId(8), grid);
    assert_eq!(
        terminal_input_target_of(&pty_app),
        Some((TerminalId(8), true))
    );
    assert!(matches!(
        MutationRequest::restart(&pty),
        Some(MutationRequest {
            body: RequestBody::RestartTerminal {
                terminal: TerminalId(8)
            },
            expected: ExpectedResponse::Terminal,
            operation: "restart terminal",
        })
    ));
}

#[test]
fn selection_motion_waits_for_scrolled_frame() {
    let terminal = TerminalId(9);
    let mut grid = MirrorGrid::new(80, 24);
    grid.primed = true;
    grid.seq = 10;
    grid.viewport.scrollback_len = 100;
    grid.viewport.offset = 20;
    grid.viewport.history_epoch = 4;
    let mut pending = None;

    PendingSelectionScroll::queue(&mut pending, terminal, &grid, -3);
    let mut pending = pending.unwrap_or_else(|| panic!("scroll motion must be retained"));
    assert_eq!(pending.reconcile(terminal, &grid), None);

    grid.seq = 11;
    assert_eq!(pending.reconcile(terminal, &grid), None);
    grid.viewport.offset = 23;
    assert_eq!(pending.reconcile(terminal, &grid), Some(-3));
}

#[test]
fn selection_motion_resolves_on_unrelated_viewport_frame() {
    let terminal = TerminalId(10);
    let mut grid = MirrorGrid::new(80, 24);
    grid.primed = true;
    grid.seq = 20;
    grid.viewport.scrollback_len = 100;
    grid.viewport.offset = 20;
    grid.viewport.history_epoch = 5;
    let mut pending = None;

    PendingSelectionScroll::queue(&mut pending, terminal, &grid, -3);
    let mut pending = pending.unwrap_or_else(|| panic!("scroll motion must be retained"));
    grid.seq = 21;
    grid.viewport.offset = 10;
    assert_eq!(pending.reconcile(terminal, &grid), Some(10));
}

#[derive(Default)]
struct RejectingMutationRequester {
    requests: RefCell<Vec<RequestBody>>,
}

impl MutationRequester for RejectingMutationRequester {
    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<Result<ResponseBody, ProtoError>> {
        self.requests.borrow_mut().push(body);
        let (reply, answer) = async_channel::bounded(1);
        reply
            .try_send(Err(ProtoError {
                kind: ErrorKind::Conflict,
                message: "terminal is busy".to_owned(),
            }))
            .unwrap_or_else(|error| panic!("test reply must be accepted: {error}"));
        answer
    }
}

#[gpui::test]
fn terminal_mutation_refusals_are_sticky(cx: &mut gpui::TestAppContext) {
    let requester = RejectingMutationRequester::default();
    let state = cx.new(|_| AppState::new("/tmp/fleet-workspace-test", Instant::now()));
    let request = MutationRequest::restart(&session("pty/session", terminal(8, TerminalKind::Pty)))
        .unwrap_or_else(|| panic!("PTY restart must produce a mutation request"));

    cx.update(|cx| request_mutation(&requester, request, state.clone(), cx));
    cx.run_until_parked();

    assert!(matches!(
        requester.requests.borrow().as_slice(),
        [RequestBody::RestartTerminal {
            terminal: TerminalId(8)
        }]
    ));
    state.read_with(cx, |app, _| {
        assert_eq!(
            app.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("terminal is busy")
        );
    });
}

#[test]
fn the_status_glyph_matches_the_hub_row() {
    assert_eq!(
        status_kind(SessionState::Attached, false, AgentActivity::Unknown, false,),
        StatusKind::Attached
    );
    assert_eq!(
        status_kind(SessionState::Detached, true, AgentActivity::Unknown, false,),
        StatusKind::Sleeping
    );
    assert_eq!(
        status_kind(SessionState::Detached, false, AgentActivity::Unknown, false,),
        StatusKind::DetachedAwake
    );
    assert_eq!(
        status_kind(SessionState::Unknown, false, AgentActivity::Unknown, false,),
        StatusKind::Unknown
    );
    assert_eq!(
        status_kind(SessionState::None, false, AgentActivity::Unknown, false,),
        StatusKind::NoSession
    );
    // A failed post-create hook outranks every session state (§2.5).
    assert_eq!(
        status_kind(SessionState::Attached, false, AgentActivity::Working, true,),
        StatusKind::Degraded
    );
    assert_eq!(
        status_kind(SessionState::Detached, true, AgentActivity::Idle, false,),
        StatusKind::AgentFinished
    );
}

#[test]
fn the_pr_badge_follows_the_strict_priority_order() {
    assert_eq!(
        badge_state(true, PrChecks::Fail, PrReviewDecision::Approved),
        PrBadgeState::Draft
    );
    assert_eq!(
        badge_state(false, PrChecks::Fail, PrReviewDecision::Approved),
        PrBadgeState::CiFail
    );
    assert_eq!(
        badge_state(false, PrChecks::Pass, PrReviewDecision::ChangesRequested),
        PrBadgeState::Changes
    );
    assert_eq!(
        badge_state(false, PrChecks::Pending, PrReviewDecision::None),
        PrBadgeState::CiPending
    );
    assert_eq!(
        badge_state(false, PrChecks::Pass, PrReviewDecision::Approved),
        PrBadgeState::Approved
    );
    assert_eq!(
        badge_state(false, PrChecks::None, PrReviewDecision::ReviewRequired),
        PrBadgeState::Review
    );
}

#[test]
fn the_scroll_caret_stops_at_both_edges() {
    // The caret is an absolute scrollback line: a viewport of 3 rows starting at line 900
    // confines it to 900..=902, and the edges are where `j` / `k` start scrolling instead.
    // `track_selection` clamps the caret into the viewport every frame, so a key only ever
    // sees one that is already in range; `move_caret_within` normalizes as a safety net.
    let surface = local_with(|local| local.caret = 900);
    let local = Rc::new(RefCell::new(surface));
    assert!(local.borrow_mut().move_caret_within(1, 900, 3));
    assert_eq!(local.borrow().caret, 901);
    assert!(local.borrow_mut().move_caret_within(5, 900, 3));
    assert_eq!(local.borrow().caret, 902);
    assert!(!local.borrow_mut().move_caret_within(1, 900, 3));
    assert!(local.borrow_mut().move_caret_within(-9, 900, 3));
    assert_eq!(local.borrow().caret, 900);
    assert!(!local.borrow_mut().move_caret_within(-1, 900, 3));
}

#[test]
fn the_caret_cannot_move_in_an_empty_grid() {
    let local = Rc::new(RefCell::new(Local::default()));
    assert!(!local.borrow_mut().move_caret_within(1, 0, 0));
}

#[test]
fn an_unmeasured_area_falls_back_to_a_conventional_grid() {
    let local = Local::default();
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(local.size_for(TerminalId(1), cell), FALLBACK_GRID);
}

#[test]
fn a_measured_area_decides_the_grid() {
    let local = local_with(|local| {
        local.area = Bounds::new(
            gpui::point(px(0.0), px(0.0)),
            gpui::size(px(216.0), px(416.0)),
        );
    });
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(local.size_for(TerminalId(1), cell), (20, 20));
}

#[test]
fn a_remembered_size_survives_an_unmeasured_frame() {
    let mut local = Local::default();
    local.sizes.insert(TerminalId(7), (100, 30));
    let cell = gpui::size(px(10.0), px(20.0));
    assert_eq!(local.size_for(TerminalId(7), cell), (100, 30));
}

#[test]
fn pre_frame_keys_and_pastes_flush_in_input_order() {
    let key = |character| {
        PendingInput::Key(KeyEvent {
            key: Key::Char(character),
            mods: Modifiers::empty(),
            text: Some(character.to_string()),
            action: KeyAction::Press,
        })
    };
    let mut pending = vec![key('a'), PendingInput::Paste("middle".to_owned()), key('b')];

    let requests = drain_pending_requests(&mut pending, TerminalId(9));

    assert!(pending.is_empty());
    assert!(matches!(
        requests.as_slice(),
        [
            RequestBody::TerminalKey {
                terminal: TerminalId(9),
                key: KeyEvent { key: Key::Char('a'), .. },
            },
            RequestBody::PasteTerminal {
                terminal: TerminalId(9),
                text,
            },
            RequestBody::TerminalKey {
                terminal: TerminalId(9),
                key: KeyEvent { key: Key::Char('b'), .. },
            },
        ] if text == "middle"
    ));
}

#[test]
fn mouse_release_only_consumes_an_active_terminal_drag() {
    let mut local = Local::default();
    // The capture-phase mouse-up-out handler must let watch tab/close clicks bubble.
    assert_eq!(end_mouse_drag(&mut local), None);
    let point = AbsoluteCellPoint::new(3, 1);
    for selected in [false, true] {
        local.mouse_selection = Some(MouseSelection {
            anchor: point,
            head: point,
            initial: AbsoluteCellSelection::new(point, point),
            initiating: point,
            granularity: SelectionGranularity::Cell,
            history_epoch: 7,
            cols: 80,
            alt_screen: false,
            dragging: true,
            selected,
        });
        assert_eq!(end_mouse_drag(&mut local), Some(selected));
        assert_eq!(local.mouse_selection.is_some(), selected);
        // Even a retained selection no longer owns subsequent releases on sibling controls.
        assert_eq!(end_mouse_drag(&mut local), None);
    }

    let point = AbsoluteCellPoint::new(3, 1);
    local.mouse_selection = Some(MouseSelection {
        anchor: point,
        head: point,
        initial: AbsoluteCellSelection::new(point, point),
        initiating: point,
        granularity: SelectionGranularity::Cell,
        history_epoch: 0,
        cols: 80,
        alt_screen: false,
        dragging: true,
        selected: true,
    });
    assert!(local.cancel_drag());
    assert!(local.mouse_selection.is_none());
    assert!(!local.cancel_drag());
}

#[test]
fn history_epoch_change_clears_selections_and_the_row_cache() {
    let point = AbsoluteCellPoint::new(3, 1);
    let mut local = local_with(|local| {
        local.anchor = Some(3);
        local.anchor_history_epoch = Some(7);
        local.mouse_selection = Some(MouseSelection {
            anchor: point,
            head: point,
            initial: AbsoluteCellSelection::new(point, point),
            initiating: point,
            granularity: SelectionGranularity::Cell,
            history_epoch: 7,
            cols: 80,
            alt_screen: false,
            dragging: false,
            selected: true,
        });
    });
    local.row_caches.insert(
        TerminalId(1),
        TerminalRowCache {
            cols: 80,
            alt_screen: false,
            history_epoch: 7,
            last_seq: 4,
            last_viewport_base: 2,
            rows: BTreeMap::new(),
        },
    );

    assert!(invalidate_history_epoch(&mut local, Some(8)));
    assert!(local.mouse_selection.is_none());
    assert!(local.anchor.is_none());
    assert!(local.row_caches.is_empty());
}

#[test]
fn unchanged_history_epoch_keeps_selections_and_the_row_cache() {
    let point = AbsoluteCellPoint::new(3, 1);
    let mut local = local_with(|local| {
        local.mouse_selection = Some(MouseSelection {
            anchor: point,
            head: point,
            initial: AbsoluteCellSelection::new(point, point),
            initiating: point,
            granularity: SelectionGranularity::Cell,
            history_epoch: 7,
            cols: 80,
            alt_screen: false,
            dragging: false,
            selected: true,
        });
    });
    local.row_caches.insert(
        TerminalId(1),
        TerminalRowCache {
            cols: 80,
            alt_screen: false,
            history_epoch: 7,
            last_seq: 4,
            last_viewport_base: 2,
            rows: BTreeMap::new(),
        },
    );

    assert!(!invalidate_history_epoch(&mut local, Some(7)));
    assert!(local.mouse_selection.is_some());
    assert_eq!(local.row_caches.len(), 1);
}

#[test]
fn row_cache_is_untouched_without_a_selection_and_retains_only_selected_rows() {
    let mut grid = MirrorGrid::new(4, 4);
    grid.seq = 3;
    let retained =
        cached_grid_row(&grid, 0).unwrap_or_else(|| panic!("an initialized mirror row must exist"));
    let mut cache = TerminalRowCache {
        cols: 4,
        alt_screen: false,
        history_epoch: 0,
        last_seq: 1,
        last_viewport_base: 99,
        rows: BTreeMap::from([(99, retained)]),
    };

    cache_selected_grid_rows(&mut cache, &grid, 100, None);
    assert_eq!(cache.rows.keys().copied().collect::<Vec<_>>(), vec![99]);
    assert_eq!((cache.last_seq, cache.last_viewport_base), (1, 99));

    cache_selected_grid_rows(&mut cache, &grid, 100, Some((102, 101)));
    assert_eq!(
        cache.rows.keys().copied().collect::<Vec<_>>(),
        vec![101, 102]
    );
    assert_eq!((cache.last_seq, cache.last_viewport_base), (3, 100));
}
