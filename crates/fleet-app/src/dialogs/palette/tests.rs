use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Instant};

use gpui::{Context, Render};

use super::*;
use super::{command::*, rows::*};

type TestReplySender = async_channel::Sender<
    Result<fleet_proto::response::ResponseBody, fleet_proto::error::ProtoError>,
>;

#[derive(Debug, PartialEq)]
enum RecordedRequest {
    Sent(RequestBody),
    Requested(RequestBody),
}

#[derive(Clone, Default)]
struct FakeTransport {
    requests: Rc<RefCell<Vec<RecordedRequest>>>,
    replies: Rc<RefCell<VecDeque<TestReplySender>>>,
}

impl SessionTransport for FakeTransport {
    fn send(&self, body: RequestBody) {
        self.requests.borrow_mut().push(RecordedRequest::Sent(body));
    }

    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<
        Result<fleet_proto::response::ResponseBody, fleet_proto::error::ProtoError>,
    > {
        let (sender, receiver) = async_channel::bounded(1);
        self.requests
            .borrow_mut()
            .push(RecordedRequest::Requested(body));
        self.replies.borrow_mut().push_back(sender);
        receiver
    }
}

struct PaletteFixture;

impl Render for PaletteFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[test]
fn a_go_row_says_its_state_not_its_type() {
    // §3.9's right-hand column is the §2.5 state; "worktree" is what the id already says.
    assert_eq!(
        session_detail(SessionState::Attached, false),
        "session attached"
    );
    assert_eq!(session_detail(SessionState::Detached, true), "sleeping");
    assert_eq!(
        session_detail(SessionState::Detached, false),
        "running, detached"
    );
    assert_eq!(session_detail(SessionState::None, false), "no session");
    assert_eq!(session_detail(SessionState::Unknown, false), "unknown");
}
#[test]
fn every_command_has_a_label_and_a_bound_key() {
    for command in Command::ALL {
        assert!(!command.info().label.is_empty());
        assert!(
            keys_for(command.action()).is_some(),
            "`{}` is bound to nothing",
            command.action()
        );
    }
}

#[test]
fn the_palette_offers_exactly_the_catalogue_s_palette_entries() {
    let commands: std::collections::HashSet<&str> = Command::ALL
        .iter()
        .map(|command| command.action())
        .collect();
    let flagged: std::collections::HashSet<&str> = crate::action_catalogue::entries()
        .iter()
        .filter(|entry| entry.info.palette)
        .map(crate::action_catalogue::Entry::action)
        .collect();
    assert_eq!(commands, flagged);
    for command in Command::ALL {
        let info = crate::action_catalogue::info(command.action())
            .unwrap_or_else(|| panic!("{command:?}"));
        assert_eq!(command.info().label, info.label);
        assert_eq!(command.destructive(), info.destructive);
    }
}

#[test]
fn destructive_commands_are_marked_and_are_the_ones_with_confirms() {
    assert!(Command::DeleteWorktree.destructive());
    assert!(Command::PruneWorktrees.destructive());
    assert!(Command::QuitDaemon.destructive());
    assert!(!Command::Settings.destructive());
}

#[test]
fn commands_that_need_a_connection_or_snapshot_are_not_listed_without_one() {
    let state = AppState::new("/tmp/fleet", Instant::now());
    assert!(!Command::NewWorktree.valid(&state));
    assert!(!Command::DeleteWorktree.valid(&state));
    assert!(!Command::Settings.valid(&state));
    assert!(Command::Help.valid(&state));
    assert!(!Command::UpdateFleet.valid(&state));
}

#[test]
fn an_empty_snapshot_still_offers_the_always_valid_commands() {
    let state = AppState::new("/tmp/fleet", Instant::now());
    let rows = candidates(&state, "", None, None, &[]);
    assert!(
        rows.is_empty(),
        "without a snapshot the palette has nothing to point at"
    );
}

fn go_snapshot(state: SessionState, slept: bool) -> fleet_proto::snapshot::Snapshot {
    use fleet_core::sessions::{Session, SessionKind};
    let id: WorktreeId = "acme/widgets#feature-one"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let mut snapshot = fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-04T12:00:00Z".to_owned(),
        revision: None,
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "0.1.0".to_owned(),
            pid: 1,
            started_at: "2026-09-04T09:00:00Z".to_owned(),
            home: "/tmp/fleet".to_owned(),
        },
    };
    snapshot.worktrees = vec![fleet_core::model::Worktree {
        id: id.clone(),
        repo_id: "acme/widgets"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        slug: "feature-one".to_owned(),
        branch: "feature-one".to_owned(),
        base_ref: "main".to_owned(),
        path: "/tmp/widgets/feature-one".to_owned(),
        session: "widgets/feature-one".to_owned(),
        host: None,
        created_at: "2026-09-04T09:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    }];
    snapshot.sessions = vec![Session {
        id: "widgets/feature-one"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        host: None,
        kind: SessionKind::Worktree(id.clone()),
        cwd: "/tmp/widgets/feature-one".to_owned(),
        terminals: Vec::new(),
        active_terminal: None,
        slept_at: slept.then(|| "2026-09-04T11:00:00Z".to_owned()),
        kept_terminals: Vec::new(),
    }];
    snapshot.statuses = vec![fleet_core::sessions::WorktreeStatus {
        worktree_id: id,
        session: state,
        windows: Vec::new(),
        running: Vec::new(),
        agent_activity: fleet_core::sessions::AgentActivity::Unknown,
        agent_activity_changed_at: None,
    }];
    snapshot
}

fn multi_session_snapshot(count: usize) -> fleet_proto::snapshot::Snapshot {
    let template = go_snapshot(SessionState::Detached, false);
    let worktree = template.worktrees[0].clone();
    let session = template.sessions[0].clone();
    let mut snapshot = template;
    snapshot.worktrees.clear();
    snapshot.sessions.clear();
    snapshot.statuses.clear();
    for index in 0..count {
        let worktree_id: WorktreeId = format!("acme/widgets#feature-{index}")
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let session_id: SessionId = format!("widgets/feature-{index}")
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        snapshot.worktrees.push(fleet_core::model::Worktree {
            id: worktree_id.clone(),
            slug: format!("feature-{index}"),
            branch: format!("feature-{index}"),
            session: session_id.as_str().to_owned(),
            ..worktree.clone()
        });
        snapshot.sessions.push(fleet_core::sessions::Session {
            id: session_id,
            kind: SessionKind::Worktree(worktree_id),
            ..session.clone()
        });
    }
    snapshot
}

fn displayed_worktrees(count: usize) -> Vec<crate::presentation::DisplayedWorktree> {
    (0..count)
        .map(|index| crate::presentation::DisplayedWorktree {
            id: format!("acme/widgets#feature-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            repo: "acme/widgets"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        })
        .collect()
}

fn picker_summary(
    worktree: &str,
    provider: fleet_core::agents::AgentKind,
    title: &str,
    parent: Option<ThreadId>,
) -> AgentThreadSummary {
    let worktree = worktree.parse().unwrap_or_else(|error| panic!("{error}"));
    let mut projection =
        fleet_core::agents::ThreadProjection::new(ThreadId::new(), worktree, provider);
    projection.title = title.to_owned();
    projection.last_activity = Some(chrono::Utc::now() - chrono::Duration::minutes(14));
    let mut summary = projection.summary(Seq::default());
    summary.parent = parent;
    summary
}

fn agents_picker_state() -> (AppState, Vec<AgentThreadSummary>) {
    let now = Instant::now();
    let mut snapshot = multi_session_snapshot(2);
    let caller = picker_summary(
        "acme/widgets#feature-0",
        fleet_core::agents::AgentKind::Claude,
        "current caller",
        None,
    );
    let closed = picker_summary(
        "acme/widgets#feature-0",
        fleet_core::agents::AgentKind::Claude,
        "closed caller",
        None,
    );
    let child = picker_summary(
        "acme/widgets#feature-0",
        fleet_core::agents::AgentKind::Codex,
        "local child",
        Some(caller.thread),
    );
    let other_child = picker_summary(
        "acme/widgets#feature-1",
        fleet_core::agents::AgentKind::Codex,
        "remote child",
        Some(caller.thread),
    );
    let unrelated = picker_summary(
        "acme/widgets#feature-1",
        fleet_core::agents::AgentKind::Claude,
        "other caller",
        None,
    );
    let summaries = vec![
        child.clone(),
        caller.clone(),
        other_child.clone(),
        closed.clone(),
        unrelated,
    ];
    snapshot.agent_threads = summaries.clone();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: "widgets/feature-0"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.agents.attach(child.thread);
    state.agents.close(closed.thread);
    (state, summaries)
}

#[test]
fn agents_picker_orders_callers_then_local_and_other_worktree_children() {
    let (state, _) = agents_picker_state();
    let rows = candidates(&state, "agents", None, None, &[]);
    assert_eq!(
        rows.iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        [
            "claude — current caller",
            "claude — closed caller",
            "↳ codex — local child",
            "↳ codex — remote child · acme/widgets#feature-1",
        ]
    );
    assert!(rows.iter().all(|row| row.section == Section::Agents));
}

#[test]
fn idle_agent_attention_has_no_picker_glyph() {
    assert!(!attention_has_visible_mark(Attention::Idle));
    assert!(attention_has_visible_mark(Attention::Working));
    assert!(attention_has_visible_mark(Attention::Failed));
}

#[test]
fn prepared_key_uses_scalar_agent_revisions() {
    let (mut state, summaries) = agents_picker_state();
    let first = PreparedKey::new(&state, String::new(), None, None);
    let same = PreparedKey::new(&state, String::new(), None, None);
    assert_eq!(first, same);

    state.agents.mark_seen(summaries[0].thread, Seq(1));
    let seen = PreparedKey::new(&state, String::new(), None, None);
    assert_eq!(seen.agent_summaries, first.agent_summaries);
    assert_ne!(seen.agent_seen, first.agent_seen);
}

#[test]
fn agents_picker_keeps_a_closed_caller_reachable() {
    let (state, summaries) = agents_picker_state();
    let closed = summaries
        .iter()
        .find(|summary| summary.title == "closed caller")
        .expect("closed caller fixture");
    assert!(state.agents.is_closed(closed.thread));
    let rows = candidates(&state, "agents", None, None, &[]);
    let row = rows
        .iter()
        .find(|row| row.label.contains("closed caller"))
        .expect("closed caller row");
    assert_eq!(key_text(row), None);
    assert_eq!(row.trailing.as_deref(), Some("go"));
}

#[test]
fn agents_picker_shows_a_strip_index_only_for_an_attached_thread() {
    let (state, _) = agents_picker_state();
    let rows = candidates(&state, "agents", None, None, &[]);
    let caller = rows
        .iter()
        .find(|row| row.label == "claude — current caller")
        .expect("caller row");
    let child = rows
        .iter()
        .find(|row| row.label.contains("local child"))
        .expect("local child row");
    let hidden = rows
        .iter()
        .find(|row| row.label.contains("remote child"))
        .expect("remote child row");
    assert_eq!(key_text(caller).as_deref(), Some("ctrl-s 1"));
    assert_eq!(key_text(child).as_deref(), Some("ctrl-s 2"));
    assert_eq!(key_text(hidden), None);
}

#[gpui::test]
fn capacity_refused_agent_selection_does_not_ensure_a_session(cx: &mut gpui::TestAppContext) {
    let (mut app, summaries) = agents_picker_state();
    let target = summaries
        .iter()
        .find(|summary| summary.title == "remote child")
        .expect("remote child fixture")
        .thread;
    let target_worktree = summaries
        .iter()
        .find(|summary| summary.thread == target)
        .expect("target summary")
        .worktree
        .clone();
    let session = app
        .snapshot
        .as_mut()
        .and_then(|snapshot| {
            snapshot.sessions.iter_mut().find(|session| {
                matches!(&session.kind, SessionKind::Worktree(worktree) if worktree == &target_worktree)
            })
        })
        .expect("target worktree session");
    while session.terminals.len() < crate::state::WORKSPACE_TAB_LIMIT - 1 {
        let index = session.terminals.len();
        session.terminals.push(fleet_core::sessions::Terminal {
            id: fleet_core::ids::TerminalId(index as u64 + 10),
            name: format!("terminal-{index}"),
            command: "shell".to_owned(),
            cwd: session.cwd.clone(),
            shell_pid: None,
            foreground_command: None,
            status: fleet_core::sessions::TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
            agent_attention: None,
            kind: fleet_core::sessions::TerminalKind::Pty,
        });
    }
    app.overlay = Some(Overlay::Palette);
    let rows = candidates(&app, "agents", None, None, &[]);
    let cursor = rows
        .iter()
        .position(|row| row.run == Run::OpenAgentThread(target))
        .expect("target palette row");
    let state = cx.new(|_| app);
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.palette.rows = rows.into();
            host.palette.cursor = cursor;
        });
    });
    let transport = FakeTransport::default();
    let window = cx.add_window(|_, _| PaletteFixture);

    window
        .update(cx, |_, window, cx| {
            run_selected(&state, &transport, window, cx)
        })
        .expect("run capacity-refused selection");

    assert!(
        transport.requests.borrow().is_empty(),
        "a full strip must not issue EnsureSession"
    );
    cx.read(|cx| assert!(!state.read(cx).agents.is_attached(target)));
}

#[gpui::test]
fn reopening_a_closed_caller_from_the_picker_sends_agent_thread_reopen(
    cx: &mut gpui::TestAppContext,
) {
    let (mut app, summaries) = agents_picker_state();
    let target = summaries
        .iter()
        .find(|summary| summary.title == "closed caller")
        .expect("closed caller fixture")
        .thread;
    app.overlay = Some(Overlay::Palette);
    let rows = candidates(&app, "agents", None, None, &[]);
    let cursor = rows
        .iter()
        .position(|row| row.run == Run::OpenAgentThread(target))
        .expect("closed caller palette row");
    let state = cx.new(|_| app);
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.palette.rows = rows.into();
            host.palette.cursor = cursor;
        });
    });
    let transport = FakeTransport::default();
    let window = cx.add_window(|_, _| PaletteFixture);

    window
        .update(cx, |_, window, cx| {
            run_selected(&state, &transport, window, cx)
        })
        .expect("run closed caller selection");

    assert_eq!(
        transport.requests.borrow().as_slice(),
        [RecordedRequest::Sent(RequestBody::AgentThreadReopen {
            thread: target
        })]
    );
    cx.read(|cx| assert!(!state.read(cx).agents.is_closed(target)));
}

#[gpui::test]
fn selecting_an_already_open_top_level_thread_from_the_picker_sends_nothing(
    cx: &mut gpui::TestAppContext,
) {
    let (mut app, summaries) = agents_picker_state();
    let target = summaries
        .iter()
        .find(|summary| summary.title == "current caller")
        .expect("current caller fixture")
        .thread;
    app.overlay = Some(Overlay::Palette);
    let rows = candidates(&app, "agents", None, None, &[]);
    let cursor = rows
        .iter()
        .position(|row| row.run == Run::OpenAgentThread(target))
        .expect("current caller palette row");
    let state = cx.new(|_| app);
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.palette.rows = rows.into();
            host.palette.cursor = cursor;
        });
    });
    let transport = FakeTransport::default();
    let window = cx.add_window(|_, _| PaletteFixture);

    window
        .update(cx, |_, window, cx| {
            run_selected(&state, &transport, window, cx)
        })
        .expect("run open caller selection");

    assert!(transport.requests.borrow().is_empty());
}

#[test]
fn agents_picker_filters_on_the_provider_name() {
    let (state, _) = agents_picker_state();
    let rows = candidates(&state, "agents codex", None, None, &[]);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.label.contains("codex")));
}

#[test]
fn agents_picker_projects_cached_attention_without_attaching_a_blocked_child() {
    let (mut state, summaries) = agents_picker_state();
    let mut child = summaries
        .iter()
        .find(|summary| summary.title == "remote child")
        .expect("remote child fixture")
        .clone();
    child.attention = Attention::NeedsYou(AttentionKind::Question);
    state.apply_agent_summary(child.clone(), Instant::now());
    let row = candidates(&state, "agents", None, None, &[])
        .into_iter()
        .find(|row| row.label.contains("remote child"))
        .expect("blocked child row");
    assert_eq!(row.secondary.as_deref(), Some("blocked · question"));
    assert_eq!(
        row.attention,
        Some(Attention::NeedsYou(AttentionKind::Question))
    );
    assert!(!state.agents.is_attached(child.thread));
}

#[test]
fn kill_targets_highlighted_session() {
    let mut app = AppState::new("/tmp/fleet", Instant::now());
    app.snapshot = Some(multi_session_snapshot(2));
    app.displayed_hub.worktrees = displayed_worktrees(2);
    app.cursors.worktrees = 1;
    let request = kill_request(&app).expect("highlighted worktree has a session");
    assert!(matches!(
        request,
        ConfirmRequest::KillSession { session, .. }
            if session.as_str() == "widgets/feature-1"
    ));

    app.screen = Screen::Workspace {
        session: "widgets/feature-0"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    assert_eq!(
        target_session(&app).map(|session| session.id.as_str()),
        Some("widgets/feature-0"),
        "the active Workspace session outranks the Hub's retained cursor"
    );
}

#[test]
fn commands_require_executable_current_targets() {
    let mut app = AppState::new("/tmp/fleet", Instant::now());
    app.snapshot = Some(multi_session_snapshot(1));
    app.displayed_hub.worktrees = displayed_worktrees(1);
    assert!(!Command::SleepSession.valid(&app));
    assert!(!Command::KillSession.valid(&app));
    assert!(!Command::Settings.valid(&app));

    app.daemon = crate::state::DaemonLink::Connected;
    assert!(Command::SleepSession.valid(&app));
    assert!(Command::KillSession.valid(&app));
    assert!(Command::Settings.valid(&app));

    app.cursors.worktrees = 9;
    assert!(!Command::SleepSession.valid(&app));
    assert!(!Command::KillSession.valid(&app));
}

/// `Open the worktree's board` dispatches a `Workspace > Prefix` action, and that handler
/// exists only while the Workspace is showing a worktree session. Listed anywhere else the
/// row would be one `Enter` that does nothing at all (§3.9 lists no row that cannot run).
#[test]
fn the_board_tab_row_needs_a_worktree_session_on_screen() {
    let mut app = AppState::new("/tmp/fleet", Instant::now());
    app.daemon = crate::state::DaemonLink::Connected;
    app.snapshot = Some(multi_session_snapshot(1));
    assert!(
        !Command::WorkspaceOpenBoard.valid(&app),
        "the Hub has no board tab to open"
    );

    app.screen = Screen::Workspace {
        session: "widgets/feature-0"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    assert!(Command::WorkspaceOpenBoard.valid(&app));

    if let Some(snapshot) = app.snapshot.as_mut() {
        snapshot.sessions[0].kind = SessionKind::Agent(fleet_core::config::Agent::Claude);
    }
    assert!(
        !Command::WorkspaceOpenBoard.valid(&app),
        "an agent session has no worktree, so it has no board"
    );
}

#[test]
fn destructive_target_matches_row() {
    let mut app = AppState::new("/tmp/fleet", Instant::now());
    app.snapshot = Some(multi_session_snapshot(2));
    app.displayed_hub.worktrees = displayed_worktrees(2).into_iter().rev().collect();
    app.cursors.worktrees = 0;

    assert_eq!(
        selected_worktree_id(&app).as_ref().map(WorktreeId::as_str),
        Some("acme/widgets#feature-1"),
        "the destructive target follows the first displayed row, not snapshot index zero"
    );
}

#[test]
fn session_switcher_is_mru_and_uncapped() {
    let mut app = AppState::new("/tmp/fleet", Instant::now());
    app.snapshot = Some(multi_session_snapshot(VISIBLE_ROWS + 4));
    app.touch_session(
        "widgets/feature-2"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    );
    app.touch_session(
        "widgets/feature-6"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    );

    let rows = candidates(&app, "sessions", None, None, &[]);
    // Every session, past the visible height: the list scrolls rather than dropping rows.
    assert_eq!(rows.len(), VISIBLE_ROWS + 4);
    assert_eq!(rows[0].label, "acme/widgets#feature-6");
    assert_eq!(rows[1].label, "acme/widgets#feature-2");
    // `@` is the same list, reached by its prefix.
    assert_eq!(
        candidates(&app, "@", None, None, &[]).len(),
        VISIBLE_ROWS + 4
    );
}

#[test]
fn a_go_row_takes_its_state_and_its_id_from_the_same_place_the_hub_does() {
    let now = Instant::now();
    let mut app = AppState::new("/tmp/fleet", now);
    app.apply_snapshot(go_snapshot(SessionState::Detached, false), now);
    let rows = candidates(&app, "", None, None, &[]);
    let go: Vec<_> = rows
        .iter()
        .filter(|entry| entry.section == Section::Recent)
        .collect();
    assert_eq!(go.len(), 1, "one worktree is one GO row, {go:?}");
    assert_eq!(
        go[0].label, "acme/widgets#feature-one",
        "\u{a7}3.9 labels a GO row with its WorktreeId, never a session id"
    );
    assert_eq!(go[0].detail.as_deref(), Some("running, detached"));
    assert_eq!(go[0].status, Some(StatusKind::DetachedAwake));

    // The same worktree, actually attached, and slept.
    let mut app = AppState::new("/tmp/fleet", now);
    app.apply_snapshot(go_snapshot(SessionState::Attached, false), now);
    let rows = candidates(&app, "", None, None, &[]);
    assert_eq!(rows[0].status, Some(StatusKind::Attached));
    let mut app = AppState::new("/tmp/fleet", now);
    app.apply_snapshot(go_snapshot(SessionState::Detached, true), now);
    let rows = candidates(&app, "", None, None, &[]);
    assert_eq!(rows[0].status, Some(StatusKind::Sleeping));
    assert_eq!(rows[0].detail.as_deref(), Some("sleeping"));
}

fn board_with_one_card() -> AppState {
    let mut state = AppState::new("/tmp/fleet-palette-board", Instant::now());
    state.screen = Screen::Hub { tab: HubTab::Board };
    let context = fleet_core::model::Context {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Work".into(),
        owners: Vec::new(),
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = fleet_core::board::new_board(&context, &context.created_at);
    let card = fleet_core::board::create_card(
        &mut board,
        &[],
        "card-1".parse().unwrap_or_else(|error| panic!("{error}")),
        fleet_core::board::CardDraft {
            title: "Fix login".into(),
            ..Default::default()
        },
        &context.created_at,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let column = board
        .statuses
        .iter()
        .position(|status| status.id == card.status_id)
        .unwrap_or_else(|| panic!("no column"));
    state.board.view = Some(fleet_core::board::BoardView {
        board,
        cards: vec![card],
        live_runs: Vec::new(),
    });
    state.board.focus = crate::state::BoardFocus { column, row: 0 };
    state
}

/// Contracts §5.5: the three run rows belong to a worktree board, and the Hub's context
/// board is the surface they are never offered on — its cards can hold no run at all.
#[test]
fn the_run_rows_are_offered_over_a_worktree_board_only() {
    let mut state = board_with_one_card();
    assert!(!Command::BoardAttachRun.valid(&state));
    assert!(!Command::BoardCancelRun.valid(&state));
    assert!(!Command::BoardRunNow.valid(&state));

    let over_context_board = card_context(&state, Some(Dialogs::CardDetail), None);
    assert!(!Command::BoardAttachRun.valid_with(&state, over_context_board));

    state.board.scope = Some(crate::state::BoardScope::Worktree(
        "acme/api#agent"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    ));
    let over_worktree_board = card_context(&state, Some(Dialogs::CardDetail), None);
    assert!(Command::BoardAttachRun.valid_with(&state, over_worktree_board));
    assert!(Command::BoardCancelRun.valid_with(&state, over_worktree_board));
    assert!(Command::BoardRunNow.valid_with(&state, over_worktree_board));
    // The detail is what makes them valid here; the Hub's own board tab does not.
    assert!(!Command::BoardRunNow.valid(&state));
}

/// The other three rows of §5.5 follow the pickers and the settings row beside them.
#[test]
fn the_link_agent_and_columns_rows_follow_the_board_they_edit() {
    let state = board_with_one_card();
    assert!(Command::BoardPickBlockedBy.valid(&state));
    assert!(Command::BoardPickAgent.valid(&state));
    assert!(Command::BoardColumns.valid(&state));

    let elsewhere = AppState::new("/tmp/fleet-palette-no-board", Instant::now());
    assert!(!Command::BoardPickBlockedBy.valid(&elsewhere));
    assert!(!Command::BoardPickAgent.valid(&elsewhere));
    assert!(!Command::BoardColumns.valid(&elsewhere));
}

/// P9-T04: every one of §5.5's rows runs its key's own code path.
///
/// `run_command` dispatches the row's action rather than calling a handler of its own, so
/// what makes the two paths one is that the name the row advertises is a name the keymap
/// binds. A row naming an action no context binds would dispatch into nothing.
#[test]
fn every_run_row_dispatches_the_action_its_key_is_bound_to() {
    let table = crate::keymap::table();
    for command in [
        Command::BoardAttachRun,
        Command::BoardCancelRun,
        Command::BoardRunNow,
        Command::BoardPickBlockedBy,
        Command::BoardPickAgent,
        Command::BoardColumns,
    ] {
        let action = command.action();
        let contexts: Vec<&str> = table
            .iter()
            .filter(|spec| spec.action == action)
            .map(|spec| spec.context)
            .collect();
        assert!(
            contexts.contains(&"Workspace > Native > Board"),
            "`{}` is offered where its key is unbound: {action}",
            command.info().label
        );
    }
}

#[test]
fn card_rows_that_could_only_do_nothing_are_not_listed() {
    let mut state = board_with_one_card();
    assert!(Command::CardDetailEditTitle.valid(&state));
    // §3.9: a row that opens a dialog and then returns is not a valid row.
    assert!(!Command::CardDetailClose.valid(&state));
    assert!(!Command::CardDetailSave.valid(&state));
    assert!(!Command::CardDetailKeepLocal.valid(&state));
    assert!(!Command::CardDetailTakeRemote.valid(&state));
    assert!(!Command::BoardOpenWorktree.valid(&state));

    // Over an open detail, closing and saving are exactly what the palette is for.
    let behind = card_context(&state, Some(Dialogs::CardDetail), None);
    assert!(Command::CardDetailClose.valid_with(&state, behind));
    assert!(Command::CardDetailSave.valid_with(&state, behind));
    assert!(!Command::CardDetailKeepLocal.valid_with(&state, behind));

    let card = &mut state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .cards[0];
    card.worktree_id = Some(
        "acme/api#wor-1"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    );
    card.conflict = Some(fleet_core::board::Conflict {
        detected_at: "2026-09-06T12:00:00Z".into(),
        remote: fleet_core::board::RemoteCard::default(),
        fields: vec!["title".into()],
    });
    assert!(Command::BoardOpenWorktree.valid(&state));
    assert!(Command::CardDetailKeepLocal.valid(&state));
    assert!(Command::CardDetailTakeRemote.valid(&state));
}

/// The card detail deliberately keeps the card it opened on when a refresh moves the
/// board's selection, so the `Card detail:` rows have to be judged against *that* card.
#[test]
fn the_card_detail_rows_follow_the_open_dialog_and_not_the_board_selection() {
    let mut state = board_with_one_card();
    let view = state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"));
    let mut second = view.cards[0].clone();
    second.id = "card-two".parse().unwrap_or_else(|error| panic!("{error}"));
    second.number = 2;
    second.conflict = Some(fleet_core::board::Conflict {
        detected_at: "2026-09-06T12:00:00Z".into(),
        remote: fleet_core::board::RemoteCard::default(),
        fields: vec!["title".into()],
    });
    let held = second.id.clone();
    view.cards.push(second);
    // The board is focused on the first card, which has no conflict; the dialog holds the
    // second, which does.
    let selection = card_context(&state, Some(Dialogs::CardDetail), None);
    assert!(!Command::CardDetailKeepLocal.valid_with(&state, selection));
    let held = card_context(&state, Some(Dialogs::CardDetail), Some(&held));
    assert!(Command::CardDetailKeepLocal.valid_with(&state, held));
    assert!(Command::CardDetailTakeRemote.valid_with(&state, held));
}

#[test]
fn opening_a_remote_issue_needs_a_link_that_carries_an_address() {
    let mut state = board_with_one_card();
    assert!(
        !Command::BoardOpenRemote.valid(&state),
        "an unlinked card has no issue to open"
    );
    let card = &mut state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .cards[0];
    card.remote = Some(fleet_core::board::RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "SP-1".into(),
        url: None,
        version: None,
        synced_at: "2026-09-06T12:00:00Z".into(),
        remote_updated_at: None,
    });
    assert!(
        !Command::BoardOpenRemote.valid(&state),
        "a backend that publishes no URL leaves nothing to open"
    );
    let card = &mut state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .cards[0];
    if let Some(remote) = card.remote.as_mut() {
        remote.url = Some("https://example.test/browse/SP-1".into());
    }
    assert!(Command::BoardOpenRemote.valid(&state));
    assert!(Command::CardDetailOpenRemote.valid(&state));
}

#[gpui::test]
fn palette_uses_normal_transition(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let snapshot = go_snapshot(SessionState::Detached, true);
    let session = snapshot.sessions[0].clone();
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(snapshot, now);
        app.overlay = Some(Overlay::Palette);
        app.terminal_mode = crate::state::TerminalMode::Scroll;
        app
    });
    cx.update(|cx| {
        let rows = candidates(state.read(cx), "sessions", None, None, &[]).into();
        with_host(&state, cx, |host| {
            host.palette.rows = rows;
            host.palette.cursor = 0;
        });
    });
    let transport = FakeTransport::default();
    let window = cx.add_window(|_, _| PaletteFixture);

    window
        .update(cx, |_, window, cx| {
            run_selected(&state, &transport, window, cx)
        })
        .expect("run palette selection");

    assert!(matches!(
        transport.requests.borrow().as_slice(),
        [
            RecordedRequest::Sent(RequestBody::TouchWorktreeOpened { id }),
            RecordedRequest::Requested(RequestBody::EnsureSession {
                worktree: Some(ensured),
                agent: None,
                sleep_previous: true,
            }),
        ] if id == ensured && id.as_str() == "acme/widgets#feature-one"
    ));
    transport
        .replies
        .borrow_mut()
        .pop_front()
        .expect("ensure reply")
        .try_send(Ok(fleet_proto::response::ResponseBody::Session(session)))
        .expect("send ensure reply");
    cx.run_until_parked();

    cx.read(|cx| {
        let app = state.read(cx);
        assert_eq!(app.overlay, None);
        assert_eq!(app.terminal_mode, crate::state::TerminalMode::Terminal);
        assert_eq!(app.mode(), crate::state::Mode::Terminal);
        assert!(matches!(
            app.screen,
            Screen::Workspace { ref session }
                if session.as_str() == "widgets/feature-one"
        ));
    });
}

#[gpui::test]
fn palette_wakes_agent_session_before_entering(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let mut snapshot = go_snapshot(SessionState::Detached, true);
    snapshot.worktrees.clear();
    snapshot.statuses.clear();
    let agent = fleet_core::config::Agent::Claude;
    let session = &mut snapshot.sessions[0];
    session.id = fleet_core::sessions::agent_session_id(agent).expect("agent session id");
    session.kind = SessionKind::Agent(agent);
    let response = session.clone();
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(snapshot, now);
        app.overlay = Some(Overlay::Palette);
        app
    });
    cx.update(|cx| {
        let rows = candidates(state.read(cx), "sessions", None, None, &[]).into();
        with_host(&state, cx, |host| {
            host.palette.rows = rows;
            host.palette.cursor = 0;
        });
    });
    let transport = FakeTransport::default();
    let window = cx.add_window(|_, _| PaletteFixture);

    window
        .update(cx, |_, window, cx| {
            run_selected(&state, &transport, window, cx)
        })
        .expect("run agent palette selection");

    assert!(matches!(
        transport.requests.borrow().as_slice(),
        [RecordedRequest::Requested(RequestBody::EnsureSession {
            worktree: None,
            agent: Some(fleet_core::config::Agent::Claude),
            sleep_previous: true,
        })]
    ));
    transport
        .replies
        .borrow_mut()
        .pop_front()
        .expect("agent ensure reply")
        .try_send(Ok(fleet_proto::response::ResponseBody::Session(response)))
        .expect("send agent ensure reply");
    cx.run_until_parked();

    cx.read(|cx| {
        assert!(matches!(state.read(cx).screen, Screen::Workspace { .. }));
    });
}

#[gpui::test]
fn palette_reports_ensure_failure(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let snapshot = go_snapshot(SessionState::Detached, true);
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(snapshot, now);
        app.overlay = Some(Overlay::Palette);
        app
    });
    cx.update(|cx| {
        let rows = candidates(state.read(cx), "sessions", None, None, &[]).into();
        with_host(&state, cx, |host| host.palette.rows = rows);
    });
    let transport = FakeTransport::default();
    let window = cx.add_window(|_, _| PaletteFixture);
    window
        .update(cx, |_, window, cx| {
            run_selected(&state, &transport, window, cx)
        })
        .expect("run palette selection");
    transport
        .replies
        .borrow_mut()
        .pop_front()
        .expect("ensure reply")
        .try_send(Err(fleet_proto::error::ProtoError {
            kind: fleet_proto::error::ErrorKind::Tmux,
            message: "session refused".to_owned(),
        }))
        .expect("send ensure refusal");
    cx.run_until_parked();

    cx.read(|cx| {
        let app = state.read(cx);
        assert_eq!(
            app.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("session refused")
        );
        assert!(matches!(app.screen, Screen::Hub { .. }));
    });
}

#[test]
fn a_leading_prefix_narrows_the_scope() {
    let scope = |query: &str| parse_query(query).scope;
    assert_eq!(scope("del"), Scope::All);
    assert_eq!(scope(">del"), Scope::Commands);
    assert_eq!(scope("@ pay"), Scope::GoTo);
    assert_eq!(scope("#FLT"), Scope::Cards);
    assert_eq!(scope("!codex"), Scope::Agents);
    assert_eq!(parse_query("> del ").needle, "del");
    // The words `^s W` and `^s d` used to seed keep working.
    assert!(parse_query("sessions").sessions_only);
    assert_eq!(parse_query("agents codex").scope, Scope::Agents);
    assert_eq!(parse_query("agents codex").needle, "codex");
    assert_eq!(parse_query("agentsx").scope, Scope::All);
}

#[test]
fn an_empty_query_lists_recent_sessions_then_suggested_commands() {
    let mut app = AppState::new("/tmp/fleet", Instant::now());
    app.snapshot = Some(multi_session_snapshot(VISIBLE_ROWS));
    app.daemon = crate::state::DaemonLink::Connected;
    let rows = candidates(&app, "", None, None, &[]);
    let recent = rows
        .iter()
        .take_while(|row| row.section == Section::Recent)
        .count();
    assert_eq!(recent, RECENT_ROWS);
    let suggested = &rows[recent..];
    assert!(!suggested.is_empty());
    assert!(suggested.len() <= SUGGESTED_ROWS);
    assert!(
        suggested
            .iter()
            .all(|row| row.section == Section::Suggested && matches!(row.run, Run::Command(_)))
    );
}

#[test]
fn a_typed_query_ranks_the_best_match_first_across_sections() {
    let mut app = AppState::new("/tmp/fleet", Instant::now());
    app.snapshot = Some(multi_session_snapshot(3));
    app.daemon = crate::state::DaemonLink::Connected;
    let rows = candidates(&app, "help", None, None, &[]);
    assert_eq!(rows[0].run, Run::Command(Command::Help));
    assert_eq!(rows[0].matches.len(), 4);
    // Rows of one section stay together, and no section is capped.
    let mut seen: Vec<Section> = Vec::new();
    for row in &rows {
        if seen.last() != Some(&row.section) {
            assert!(
                !seen.contains(&row.section),
                "{:?} split in two",
                row.section
            );
            seen.push(row.section);
        }
    }
    let commands = candidates(&app, ">", None, None, &[]);
    assert!(commands.iter().all(|row| row.section == Section::Commands));
    let valid = Command::ALL
        .iter()
        .filter(|command| command.valid(&app))
        .count();
    assert_eq!(
        commands.len(),
        valid,
        "a scoped list is every valid command, uncapped"
    );
}

#[test]
fn destructive_commands_say_they_ask_first_and_every_command_shows_its_key() {
    let rows: Vec<Entry> = Command::ALL
        .iter()
        .copied()
        .map(rows::command_entry)
        .collect();
    for row in &rows {
        if row.destructive {
            assert_eq!(row.detail.as_deref(), Some(ASKS_FIRST), "{}", row.label);
        }
        assert!(row.key.is_some(), "{} has no key chip", row.label);
    }
}

#[test]
fn cards_and_pull_requests_are_listed_under_their_own_sections() {
    let app = board_with_one_card();
    let cards = candidates(&app, "#", None, None, &[]);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].section, Section::Cards);
    assert!(matches!(cards[0].run, Run::OpenCard(_)));
    assert!(cards[0].badge.is_some(), "a card row names its column");

    let pr = PalettePr {
        tab: PrTab::Mine,
        repo: "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
        number: 412,
        title: "Fix RUT validation".to_owned(),
        local: None,
    };
    let rows = candidates(&app, "rut valid", None, None, std::slice::from_ref(&pr));
    let row = rows
        .iter()
        .find(|row| row.section == Section::PullRequests)
        .unwrap_or_else(|| panic!("no PR row in {rows:?}"));
    assert_eq!(row.label, "#412 Fix RUT validation");
    assert!(matches!(row.run, Run::GoToPr { number: 412, .. }));
}

#[gpui::test]
fn a_shrinking_refresh_clamps_the_palette_cursor(cx: &mut gpui::TestAppContext) {
    // §3.9: `Enter` runs the highlighted row. A snapshot that loses sessions under an
    // open palette must not leave the cursor past the last row, where `Enter` is inert.
    let now = Instant::now();
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(multi_session_snapshot(6), now);
        app.overlay = Some(Overlay::Palette);
        app.palette_seed = Some("sessions".into());
        app
    });
    cx.update(|cx| {
        seed(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.palette.rows.len(), 6);
            host.palette.cursor = 5;
        });
    });

    cx.update(|cx| {
        state.update(cx, |app, _| {
            app.apply_snapshot(multi_session_snapshot(2), now);
        });
        refresh(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.palette.rows.len(), 2);
            assert!(
                host.palette.rows.get(host.palette.cursor).is_some(),
                "the cursor still selects a row, cursor {} of {} rows",
                host.palette.cursor,
                host.palette.rows.len()
            );
        });
    });
}

#[gpui::test]
fn an_unrelated_notify_does_not_rebuild_the_palette(cx: &mut gpui::TestAppContext) {
    // `host::watch` refreshes the open palette from every `AppState` notification, and a
    // busy workspace notifies several times a second. Rebuilding the candidates there
    // indexes the whole snapshot and allocates a label and a detail per row on the
    // foreground thread, which is the projection work `docs/APP-CONTRACTS.md` keeps out of
    // the per-frame path: a notification that moved none of the inputs must not do it.
    let now = Instant::now();
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(multi_session_snapshot(3), now);
        app.overlay = Some(Overlay::Palette);
        app.palette_seed = Some("sessions".into());
        app
    });
    cx.update(|cx| {
        seed(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.palette.rows.len(), 3);
            // A row no rebuild could ever produce: it survives exactly as long as
            // `refresh` returns without calling `candidates` again.
            host.palette.rows = std::rc::Rc::from(vec![Entry {
                section: Section::Commands,
                label: "sentinel".to_owned(),
                search: None,
                badge: None,
                matches: Vec::new(),
                detail: None,
                secondary: None,
                trailing: None,
                key: None,
                destructive: false,
                icon: Icon::Boxes,
                status: None,
                attention: None,
                run: Run::Command(Command::Help),
            }]);
        });
    });

    cx.update(|cx| {
        refresh(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(
                host.palette
                    .rows
                    .iter()
                    .map(|row| row.label.as_str())
                    .collect::<Vec<_>>(),
                ["sentinel"],
                "a notify that changed nothing rebuilt the candidates"
            );
        });
    });

    cx.update(|cx| {
        // A snapshot the rows *are* derived from still rebuilds them.
        state.update(&mut *cx, |app, _| {
            app.apply_snapshot(multi_session_snapshot(4), now);
        });
        refresh(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.palette.rows.len(), 4);
        });
    });
}

#[gpui::test]
fn a_refresh_that_changes_nothing_keeps_the_prepared_rows(cx: &mut gpui::TestAppContext) {
    // The open palette is refreshed from every `AppState` notification, so a rebuild that
    // lands on the same rows must not swap the prepared list out from under the card.
    let now = Instant::now();
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(multi_session_snapshot(3), now);
        app.overlay = Some(Overlay::Palette);
        app.palette_seed = Some("sessions".into());
        app
    });
    let before = cx.update(|cx| {
        seed(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.palette.rows.len(), 3);
            host.palette.rows.clone()
        })
    });

    cx.update(|cx| {
        refresh(&state, cx);
        let after = with_host(&state, cx, |host| host.palette.rows.clone());
        assert!(
            std::rc::Rc::ptr_eq(&before, &after),
            "unchanged rows are kept, not rebuilt into a fresh Rc"
        );
    });
}

#[gpui::test]
fn palette_seeding_and_cursor_motion_reuse_prepared_matches(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/palette", std::time::Instant::now());
        state.snapshot = Some(go_snapshot(SessionState::Detached, false));
        state.palette_seed = Some("sessions".into());
        state
    });
    let before = cx.update(|cx| {
        seed(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.palette.query, "sessions");
            assert!(!host.palette.rows.is_empty());
            host.palette.rows.clone()
        })
    });
    cx.update(|cx| {
        move_cursor(&state, 1, cx);
        let after = with_host(&state, cx, |host| host.palette.rows.clone());
        assert!(std::rc::Rc::ptr_eq(&before, &after));
        with_host(&state, cx, |host| host.palette.query.push_str("missing"));
        super::super::notify(&state, cx);
        let after = with_host(&state, cx, |host| host.palette.rows.clone());
        assert!(after.is_empty());
        assert!(!std::rc::Rc::ptr_eq(&before, &after));
    });
}

/// A row's key chips in keymap spelling, for assertions.
fn key_text(entry: &Entry) -> Option<String> {
    entry.key.as_ref().map(|keys| {
        keys.iter()
            .map(Keystroke::unparse)
            .collect::<Vec<_>>()
            .join(" ")
    })
}
