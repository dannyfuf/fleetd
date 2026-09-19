use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::AppContext as _;
use gpui::{Context, Render, Window, div};

use super::*;

type AgentReply = Result<ResponseBody, fleet_proto::error::ProtoError>;

#[derive(Clone, Default)]
struct RecordingRequester {
    requests: Rc<Cell<usize>>,
    command: Rc<RefCell<Option<BridgeCommand>>>,
    pending_reply: Rc<RefCell<Option<async_channel::Sender<AgentReply>>>>,
}

impl AgentThreadRequester for RecordingRequester {
    fn request_agent(
        &self,
        command: BridgeCommand,
    ) -> async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>> {
        self.requests.set(self.requests.get() + 1);
        self.command.replace(Some(command));
        let (reply, answer) = async_channel::bounded(1);
        self.pending_reply.replace(Some(reply));
        answer
    }
}

#[gpui::test]
fn an_in_flight_open_thread_does_not_retain_app_state(cx: &mut gpui::TestAppContext) {
    let requester = RecordingRequester::default();
    let state = cx.new(|_| AppState::new("/tmp/fleet-agent-open-release", Instant::now()));
    let weak_state = state.downgrade();

    cx.update(|cx| open_thread(&requester, &state, ThreadId::new(), None, cx));
    drop(state);
    cx.run_until_parked();
    // Resources are retained until the end of the effect cycle, so one empty update flushes it.
    cx.update(|_| {});

    weak_state.assert_released();
}

#[gpui::test]
fn create_thread_leaves_model_and_mode_for_the_daemon_defaults(cx: &mut gpui::TestAppContext) {
    let requester = RecordingRequester::default();
    let worktree: WorktreeId = "fleet/app#defaults"
        .parse()
        .unwrap_or_else(|error| panic!("invalid test worktree: {error}"));
    let session_id = SessionId::try_from("fleet/app/defaults")
        .unwrap_or_else(|error| panic!("invalid test session: {error}"));
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet-agent-defaults", Instant::now());
        app.screen = Screen::Workspace {
            session: session_id.clone(),
        };
        app.snapshot = Some(fleet_proto::snapshot::Snapshot {
            boards: Vec::new(),
            generated_at: String::new(),
            revision: None,
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: vec![Session {
                id: session_id,
                host: None,
                kind: SessionKind::Worktree(worktree.clone()),
                cwd: "/tmp".to_owned(),
                terminals: Vec::new(),
                active_terminal: None,
                slept_at: None,
                kept_terminals: Vec::new(),
            }],
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "test".to_owned(),
                pid: 1,
                started_at: String::new(),
                home: "/tmp/fleet-agent-defaults".to_owned(),
            },
        });
        app
    });

    cx.update(|cx| create_thread(&requester, &state, AgentKind::Claude, cx));
    assert!(matches!(
        requester.command.borrow().as_ref(),
        Some(BridgeCommand::AgentThreadCreate {
            model: None,
            mode: None,
            ..
        })
    ));
}

#[gpui::test]
fn create_thread_without_an_active_worktree_records_the_specific_refusal_and_fallback(
    cx: &mut gpui::TestAppContext,
) {
    let requester = RecordingRequester::default();
    let state = cx.new(|_| AppState::new("/tmp/fleet-agent-create", Instant::now()));

    cx.update(|cx| create_thread(&requester, &state, AgentKind::Claude, cx));

    assert_eq!(requester.requests.get(), 0);
    state.read_with(cx, |app, _| {
        let message = app
            .sticky_error
            .as_ref()
            .map(|error| error.text.as_str())
            .unwrap_or_else(|| panic!("thread creation refusal was not shown"));
        assert!(message.contains("no active session"), "{message}");
        assert!(message.contains("ctrl-s F"), "{message}");
    });

    let legacy = cx.new(|_| {
        let session_id = SessionId::try_from("repository/claude")
            .unwrap_or_else(|error| panic!("invalid test session: {error}"));
        let mut app = AppState::new("/tmp/fleet-agent-create", Instant::now());
        app.screen = Screen::Workspace {
            session: session_id.clone(),
        };
        app.snapshot = Some(fleet_proto::snapshot::Snapshot {
            boards: Vec::new(),
            generated_at: String::new(),
            revision: None,
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: vec![Session {
                id: session_id,
                host: None,
                kind: SessionKind::Agent(fleet_core::config::Agent::Claude),
                cwd: "/tmp".to_owned(),
                terminals: Vec::new(),
                active_terminal: None,
                slept_at: None,
                kept_terminals: Vec::new(),
            }],
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "test".to_owned(),
                pid: 1,
                started_at: String::new(),
                home: "/tmp/fleet-agent-create".to_owned(),
            },
        });
        app
    });

    cx.update(|cx| create_thread(&requester, &legacy, AgentKind::Codex, cx));

    assert_eq!(requester.requests.get(), 0);
    legacy.read_with(cx, |app, _| {
        let message = app
            .sticky_error
            .as_ref()
            .map(|error| error.text.as_str())
            .unwrap_or_else(|| panic!("legacy-session refusal was not shown"));
        assert!(
            message.contains("legacy repository-level agent session"),
            "{message}"
        );
        assert!(message.contains("no worktree"), "{message}");
        assert!(message.contains("ctrl-s F"), "{message}");
    });
}

#[gpui::test]
fn a_created_thread_activates_its_owning_worktree_after_navigation(cx: &mut gpui::TestAppContext) {
    let worktree: WorktreeId = "fleet/app#feedback"
        .parse()
        .unwrap_or_else(|error| panic!("invalid test worktree: {error}"));
    let thread = ThreadId::new();
    let summary = ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude)
        .summary(fleet_core::agents::Seq::default());
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet-agent-activate", Instant::now());
        app.agents.apply_summary(summary);
        app
    });

    cx.update(|cx| activate_agent_tab(&state, thread, cx));

    state.read_with(cx, |app, _| {
        assert_eq!(app.agents.active(&worktree), Some(thread));
        assert_eq!(app.agents.of_worktree(&worktree).len(), 1);
        assert!(app.sticky_error.is_none());
    });
}

/// One durable delegation of `caller` onto `child`, live and undelivered.
fn delegation_fixture(caller: ThreadId, child: ThreadId) -> fleet_core::agents::Delegation {
    fleet_core::agents::Delegation {
        id: fleet_core::agents::DelegationId::new(),
        caller,
        caller_turn: fleet_core::agents::TurnId::new(),
        caller_item: fleet_core::agents::ItemId::new(),
        child,
        provider: AgentKind::Codex,
        depth: 1,
        brief: "verify the payroll reducer".to_owned(),
        expectation: "report the failing cases".to_owned(),
        eager: false,
        status: fleet_core::agents::DelegationStatus::Running,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: fleet_core::agents::DeliveryState::Pending,
        created: chrono::Utc::now(),
        finished: None,
        headline: None,
    }
}

/// The caller's app state: both threads listed, the delegation mirrored.
fn delegating_state(
    cx: &mut gpui::TestAppContext,
    worktree: &WorktreeId,
    record: &fleet_core::agents::Delegation,
) -> Entity<AppState> {
    let caller = record.caller;
    let child = record.child;
    let record = record.clone();
    let worktree = worktree.clone();
    cx.new(move |_| {
        let mut app = AppState::new("/tmp/fleet-delegation-row", Instant::now());
        app.agents.apply_summary(
            ThreadProjection::new(caller, worktree.clone(), AgentKind::Claude)
                .summary(fleet_core::agents::Seq::default()),
        );
        let mut child_projection = ThreadProjection::new(child, worktree.clone(), AgentKind::Codex);
        child_projection.parent = Some(caller);
        app.agents
            .apply_summary(child_projection.summary(fleet_core::agents::Seq::default()));
        app.agents.apply_delegation(record);
        app
    })
}

/// A caller transcript holding exactly one delegation row, with that row focused.
fn focused_caller_view(
    cx: &mut gpui::TestAppContext,
    worktree: &WorktreeId,
    record: &fleet_core::agents::Delegation,
) -> Entity<AgentThreadView> {
    let mut projection = ThreadProjection::new(record.caller, worktree.clone(), AgentKind::Claude);
    projection.session = fleet_core::agents::SessionState::Ready;
    projection.items = vec![fleet_core::agents::Item {
        id: record.caller_item,
        turn: record.caller_turn,
        parent: None,
        kind: fleet_core::agents::ItemKind::Delegation {
            id: record.id,
            provider: record.provider,
            child: record.child,
            status: record.status,
        },
        status: fleet_core::agents::ItemStatus::Completed,
        children: Vec::new(),
        started: record.created,
        ended: Some(record.created),
    }];
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    let delegations = vec![record.clone()];
    view.update(cx, |view, cx| {
        view.sync_delegations(delegations, HashMap::new(), 1, cx);
        view.set_scroll_mode(true, cx);
        // Row focus is what makes `⏎` / `x` / `y` fire, and it needs no laid-out window.
        view.move_row_focus(0, cx);
    });
    view
}

/// `⏎` on a delegation row is `AttachChild`, never `ExpandRow`: it brings the child into the
/// worktree's strip and selects it, so one key reaches the work instead of unfolding a summary.
#[gpui::test]
fn enter_on_a_delegation_row_attaches_and_selects_the_child(cx: &mut gpui::TestAppContext) {
    let worktree: WorktreeId = "fleet/app#delegation-attach"
        .parse()
        .unwrap_or_else(|error| panic!("invalid test worktree: {error}"));
    let record = delegation_fixture(ThreadId::new(), ThreadId::new());
    let state = delegating_state(cx, &worktree, &record);
    let view = focused_caller_view(cx, &worktree, &record);

    let focused = view.read_with(cx, |view, cx| view.focused_delegation(cx));
    assert_eq!(
        focused,
        Some(record.id),
        "the focused row must resolve to the delegation the handlers act on"
    );

    let attached = cx.update(|cx| attach_delegation_child(&state, record.id, cx));

    assert_eq!(attached, Some(record.child));
    state.read_with(cx, |app, _| {
        assert!(app.agents.is_attached(record.child));
        assert_eq!(app.agents.active(&worktree), Some(record.child));
    });
}

#[gpui::test]
fn selecting_an_attached_child_sends_no_agent_thread_reopen(cx: &mut gpui::TestAppContext) {
    let worktree: WorktreeId = "fleet/app#delegation-no-reopen"
        .parse()
        .unwrap_or_else(|error| panic!("invalid test worktree: {error}"));
    let record = delegation_fixture(ThreadId::new(), ThreadId::new());
    let state = delegating_state(cx, &worktree, &record);
    let commands = RefCell::new(Vec::new());

    cx.update(|cx| {
        assert_eq!(
            attach_delegation_child(&state, record.id, cx),
            Some(record.child)
        );
        assert!(reopen_agent_tab(
            &state,
            record.child,
            |command| commands.borrow_mut().push(command),
            cx,
        ));
    });

    assert!(commands.into_inner().is_empty());
}

/// `x` on a delegation row never cancels on the spot: it names the child in the Confirm dialog
/// first, and `dialogs::confirm` sends `DelegationCancel` only once that is accepted.
#[gpui::test]
fn x_on_a_delegation_row_opens_the_confirm_for_that_child(cx: &mut gpui::TestAppContext) {
    let worktree: WorktreeId = "fleet/app#delegation-cancel"
        .parse()
        .unwrap_or_else(|error| panic!("invalid test worktree: {error}"));
    let record = delegation_fixture(ThreadId::new(), ThreadId::new());
    let state = delegating_state(cx, &worktree, &record);
    let view = focused_caller_view(cx, &worktree, &record);

    let focused = view.read_with(cx, |view, cx| view.focused_delegation(cx));
    assert_eq!(focused, Some(record.id));

    let staged = cx.update(|cx| open_delegation_cancel_confirm(&state, record.id, cx));

    assert_eq!(staged, Some(record.child));
    state.read_with(cx, |app, _| {
        assert!(
            matches!(app.overlay, Some(Overlay::Dialog(Dialogs::Confirm))),
            "cancelling a child goes through the confirm dialog"
        );
        assert!(
            !app.agents.is_attached(record.child),
            "asking to cancel never attaches the child"
        );
    });
}

struct ComposerFocusFixture;

impl Render for ComposerFocusFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

#[gpui::test]
fn an_activated_thread_focuses_its_mounted_composer_on_the_next_frame(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let worktree: WorktreeId = "fleet/app#composer-focus"
        .parse()
        .unwrap_or_else(|error| panic!("invalid test worktree: {error}"));
    let session_id = SessionId::try_from("fleet/composer-focus")
        .unwrap_or_else(|error| panic!("invalid test session: {error}"));
    let projection = ThreadProjection::new(ThreadId::new(), worktree.clone(), AgentKind::Claude);
    let thread = projection.thread;
    let state = cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet-agent-focus", Instant::now());
        app.screen = Screen::Workspace {
            session: session_id.clone(),
        };
        app.snapshot = Some(fleet_proto::snapshot::Snapshot {
            boards: Vec::new(),
            generated_at: String::new(),
            revision: None,
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: vec![Session {
                id: session_id,
                host: None,
                kind: SessionKind::Worktree(worktree.clone()),
                cwd: "/tmp".to_owned(),
                terminals: Vec::new(),
                active_terminal: None,
                slept_at: None,
                kept_terminals: Vec::new(),
            }],
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "test".to_owned(),
                pid: 1,
                started_at: String::new(),
                home: "/tmp/fleet-agent-focus".to_owned(),
            },
        });
        app.agents
            .apply_summary(projection.summary(fleet_core::agents::Seq::default()));
        app.agents.activate(worktree, thread);
        app
    });
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    let scheduled_view = view.clone();
    let scheduled_state = state.clone();
    let window = cx.add_window(move |window, cx| {
        assert!(scheduled_state.update(cx, |app, _| { app.agents.take_composer_focus(thread) }));
        focus_composer_after_mount(scheduled_view.clone(), thread, scheduled_state, window);
        ComposerFocusFixture
    });

    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.draw(
        gpui::Point::default(),
        gpui::size(px(800.0), px(600.0)),
        |_, _| div().size_full().child(view.clone()),
    );
    visual.update(|window, cx| assert!(window.simulate_next_frame(cx) >= 1));

    visual.update(|window, cx| {
        let input = view.read(cx).input().clone();
        assert!(
            input.read(cx).focus_handle().is_focused(window),
            "the mounted composer input must hold the exact focused handle"
        );
        assert_eq!(
            state.read(cx).harness_snapshot().focused.as_deref(),
            Some("agents.composer")
        );
    });
}
