use std::cell::Cell;

use gpui::AppContext as _;

use super::*;

#[derive(Default)]
struct RecordingRequester {
    requests: Cell<usize>,
}

impl AgentThreadRequester for RecordingRequester {
    fn request_agent(
        &self,
        _command: BridgeCommand,
    ) -> async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>> {
        self.requests.set(self.requests.get() + 1);
        let (_reply, answer) = async_channel::bounded(1);
        answer
    }
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
