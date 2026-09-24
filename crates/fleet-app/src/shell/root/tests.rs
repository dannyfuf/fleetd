use super::{Shell, first_run_import_allowed, focus::*};
use crate::{
    actions::{fleet::OpenJobs, native_agent, prefix},
    dialogs::Dialogs,
    state::{
        AppState, DaemonLink, DaemonLossReason, HubPane, HubTab, Overlay, Screen, TerminalMode,
    },
};
use fleet_core::{
    board::{BoardView, CardDraft, create_card, new_board},
    config::Agent,
    ids::RepoId,
    model::{Context as FleetContext, Repo, RepoHooks, Worktree},
};
use fleet_proto::snapshot::{DaemonInfo, Snapshot};
use gpui::{Action, Entity, FocusHandle, KeyDownEvent, Keystroke, VisualTestContext};
use std::{cell::RefCell, rc::Rc, time::Instant};

fn key_down(keys: &str) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}")),
        is_held: false,
        prefer_character_input: false,
    }
}

fn queue(keys: &mut FocusOwnerKeys, event: KeyDownEvent) -> Option<KeyDownEvent> {
    keys.await_capture();
    keys.capture(event)
}

fn rendered(keys: &mut FocusOwnerKeys, generation: u64) -> Vec<KeyDownEvent> {
    keys.finish_render(generation)
}

/// The interceptor's own shape: read the live chain once, then resolve against it.
fn live_prefix(state: &mut AppState, keystroke: &Keystroke) -> (bool, Option<Box<dyn Action>>) {
    let chain = state.context_chain();
    take_live_prefix_action(state, &chain, keystroke)
}

#[test]
fn stale_keys_wait_for_their_focus_owner_generation_to_render() {
    let mut keys = FocusOwnerKeys::new(vec!["Hub", "Worktrees"]);
    let key = key_down("j");

    assert!(keys.is_stale(), "the initial tree has not painted");
    assert!(queue(&mut keys, key.clone()).is_none());
    assert!(rendered(&mut keys, 0).is_empty());
    assert_eq!(rendered(&mut keys, 1), vec![key]);
    assert!(!keys.is_stale());
    for _ in 0..100 {
        keys.sync_owner(["Hub", "Worktrees"]);
        assert!(
            !keys.should_queue(),
            "idle renders do not need another frame callback"
        );
    }

    let (generation, _) = keys.sync_owner(vec!["Agent", "Terminal"]);
    let key = key_down("x");
    assert!(queue(&mut keys, key.clone()).is_none());
    assert!(rendered(&mut keys, generation - 1).is_empty());
    assert_eq!(rendered(&mut keys, generation), vec![key]);
}

#[test]
fn stale_key_queue_is_bounded_and_drops_the_oldest_key() {
    let mut keys = FocusOwnerKeys::new(vec!["Hub", "Worktrees"]);
    for index in 0..=STALE_KEY_CAPACITY {
        let mut event = key_down("a");
        event.keystroke.key = format!("key-{index}");
        let dropped = queue(&mut keys, event);
        if index < STALE_KEY_CAPACITY {
            assert!(dropped.is_none());
        } else {
            assert_eq!(
                dropped.map(|event| event.keystroke.key),
                Some("key-0".to_owned())
            );
        }
    }
    assert_eq!(keys.queued.len(), STALE_KEY_CAPACITY);
    assert_eq!(
        keys.queued
            .front()
            .map(|event| event.keystroke.key.as_str()),
        Some("key-1")
    );
}

#[test]
fn stale_key_queue_preserves_keydown_metadata() {
    let mut keys = FocusOwnerKeys::new(vec!["Workspace", "Terminal"]);
    let mut event = key_down("a");
    event.keystroke.key_char = Some("á".to_owned());
    event.is_held = true;
    event.prefer_character_input = true;

    assert!(queue(&mut keys, event.clone()).is_none());
    assert_eq!(rendered(&mut keys, 1), vec![event]);
}

#[test]
fn replay_is_drained_before_dispatch_and_later_keys_requeue_after_a_transition() {
    let mut keys = FocusOwnerKeys::new(vec!["Workspace", "Terminal"]);
    let first = key_down("ctrl-s");
    let second = key_down("]");
    queue(&mut keys, first.clone());
    queue(&mut keys, second.clone());
    let replay = rendered(&mut keys, 1);
    assert_eq!(replay, vec![first, second.clone()]);
    assert!(
        keys.queued.is_empty(),
        "the Shell lease drains the batch before dispatch begins"
    );

    // Simulate the first replay entering Prefix before the callback dispatches the second.
    let (prefix_generation, _) = keys.sync_owner(vec!["Workspace", "Prefix"]);
    assert!(keys.is_stale());
    queue(&mut keys, replay[1].clone());
    assert_eq!(rendered(&mut keys, prefix_generation), vec![second]);
}

#[test]
fn workspace_prefix_consumes_bound_and_unbound_keys_with_daemon_banner() {
    let mut state = AppState::new("/tmp/fleet", Instant::now());
    state.screen = Screen::Workspace {
        session: "owner/repo"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.daemon = DaemonLink::Lost {
        attempt: 1,
        dismissed: false,
        reason: DaemonLossReason::ConnectionLost,
    };

    state.enter_prefix();
    let bound = Keystroke::parse("c").unwrap_or_else(|error| panic!("{error}"));
    let (consumed, action) = live_prefix(&mut state, &bound);
    assert!(consumed);
    assert_eq!(
        action.as_ref().map(|action| action.name()),
        Some(Action::name(&prefix::NewTerminal))
    );
    assert_eq!(state.terminal_mode, TerminalMode::Terminal);

    for (keys, expected) in [
        ("a", Action::name(&native_agent::NewClaude)),
        ("A", Action::name(&native_agent::NewCodex)),
        ("v", Action::name(&prefix::ToggleWatchPane)),
        ("V", Action::name(&prefix::DismissWatch)),
        ("N", Action::name(&prefix::NextWatch)),
        ("P", Action::name(&prefix::PrevWatch)),
        ("d", Action::name(&prefix::AgentsPicker)),
        ("b", Action::name(&prefix::OpenBoard)),
    ] {
        state.enter_prefix();
        let keystroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = live_prefix(&mut state, &keystroke);
        assert!(consumed, "{keys} must consume the live one-shot prefix");
        assert_eq!(action.as_ref().map(|action| action.name()), Some(expected));
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    }

    state.enter_prefix();
    let unbound = Keystroke::parse("e").unwrap_or_else(|error| panic!("{error}"));
    let (consumed, action) = live_prefix(&mut state, &unbound);
    assert!(consumed);
    assert!(action.is_none());
    assert_eq!(state.terminal_mode, TerminalMode::Terminal);
}

#[test]
fn agent_prefix_consumes_bound_and_unbound_keys_and_restores_scroll_with_daemon_banner() {
    let mut state = AppState::new("/tmp/fleet", Instant::now());
    state.toggle_agent_popup(Agent::Claude, None);
    state.agent_popup.as_mut().expect("popup open").mode = crate::state::AgentPopupMode::Scroll;
    state.daemon = DaemonLink::Lost {
        attempt: 1,
        dismissed: false,
        reason: DaemonLossReason::ConnectionLost,
    };

    state.enter_agent_prefix();
    let bound = Keystroke::parse("]").unwrap_or_else(|error| panic!("{error}"));
    let (consumed, action) = live_prefix(&mut state, &bound);
    assert!(consumed);
    assert_eq!(
        action.as_ref().map(|action| action.name()),
        Some(Action::name(&prefix::Paste))
    );
    assert_eq!(
        state.agent_popup.as_ref().map(|popup| popup.mode),
        Some(crate::state::AgentPopupMode::Scroll)
    );

    state.enter_agent_prefix();
    let unbound = Keystroke::parse("d").unwrap_or_else(|error| panic!("{error}"));
    let (consumed, action) = live_prefix(&mut state, &unbound);
    assert!(consumed);
    assert!(action.is_none());
    assert_eq!(
        state.agent_popup.as_ref().map(|popup| popup.mode),
        Some(crate::state::AgentPopupMode::Scroll)
    );
}

/// The `^s` chord of a native agent tab is taken whole, so GPUI never holds pending input.
///
/// GPUI replays the keystrokes of a sequence that matched nothing as *input*, which is how an
/// unbound `^s <key>` used to type a stray character into the composer. Arming on `^s` and
/// consuming the second key — bound or not — is what takes that path away.
#[test]
fn a_native_agent_chord_runs_its_row_and_swallows_an_unbound_second_key() {
    let chord = |chain: &[&'static str], armed: bool, keys: &str| {
        let keystroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
        agent_chord(chain, armed, &keystroke)
    };

    for chain in [
        ["Agent", "AgentIdle"],
        ["Agent", "AgentWorking"],
        ["Agent", "AgentNativeScroll"],
    ] {
        assert!(matches!(chord(&chain, false, "ctrl-s"), AgentChord::Armed));
        assert!(matches!(chord(&chain, false, "s"), AgentChord::Passthrough));
        match chord(&chain, true, "s") {
            AgentChord::Run(action) => assert_eq!(action.name(), Action::name(&prefix::GoHub)),
            other => panic!("`^s s` must reach the Hub from {chain:?}: {other:?}"),
        }
        assert!(
            matches!(chord(&chain, true, "q"), AgentChord::Unbound),
            "an unbound second key is consumed, never replayed into the composer"
        );
    }

    // …and says so, because a key that does nothing and reports nothing reads as a broken app.
    assert_eq!(
        unbound_chord_toast(&Keystroke::parse("q").unwrap_or_else(|error| panic!("{error}"))),
        "^s q is not bound here"
    );
    assert_eq!(
        unbound_chord_toast(&Keystroke::parse("ctrl-s").unwrap_or_else(|error| panic!("{error}"))),
        "^s ^s is not bound here"
    );
    // `^s s` and `^s S` are two different rows, so the refusal must not fold their case.
    assert_eq!(
        unbound_chord_toast(&Keystroke::parse("Q").unwrap_or_else(|error| panic!("{error}"))),
        "^s Q is not bound here"
    );
    assert_eq!(
        unbound_chord_toast(&Keystroke::parse("escape").unwrap_or_else(|error| panic!("{error}"))),
        "^s esc is not bound here"
    );

    // The gate contexts are three words long and the daemon banner is appended innermost;
    // neither shape may cost the tab its session rows.
    match chord(&["Agent", "AgentDecision", "AgentPlan"], true, "J") {
        AgentChord::Run(action) => assert_eq!(action.name(), Action::name(&OpenJobs)),
        other => panic!("`^s J` must open the jobs panel over a plan card: {other:?}"),
    }
    match chord(&["Agent", "AgentIdle", "Daemon", "Banner"], true, "z") {
        AgentChord::Run(action) => assert_eq!(action.name(), Action::name(&prefix::ToggleZoom)),
        other => panic!("the banner owns no `^s` row: {other:?}"),
    }

    // The legacy popup keeps its own one-shot Prefix mode, and nothing else is a chord at all.
    for chain in [
        vec!["Agent", "Terminal"],
        vec!["Agent", "Prefix"],
        vec!["Workspace", "Terminal"],
        vec!["Hub", "Worktrees"],
    ] {
        assert!(matches!(
            chord(&chain, false, "ctrl-s"),
            AgentChord::Passthrough
        ));
        assert!(matches!(chord(&chain, true, "s"), AgentChord::Passthrough));
        assert!(!is_native_agent_chain(&chain), "{chain:?}");
    }
}

#[test]
fn daemon_banner_bindings_resolve_before_live_prefix_fallback() {
    let mut state = AppState::new("/tmp/fleet", Instant::now());
    state.screen = Screen::Workspace {
        session: "owner/repo"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    };
    state.daemon = DaemonLink::Lost {
        attempt: 1,
        dismissed: false,
        reason: DaemonLossReason::ConnectionLost,
    };

    for key in ["r", "l", "escape"] {
        state.enter_prefix();
        let keystroke = Keystroke::parse(key).unwrap_or_else(|error| panic!("{error}"));
        assert!(!live_prefix(&mut state, &keystroke).0);
        assert_eq!(state.terminal_mode, TerminalMode::Prefix);
    }
}

#[test]
fn pointer_gate_rejects_stale_frames_and_base_frames_behind_live_agent() {
    let mut keys = FocusOwnerKeys::new(vec!["Workspace", "Terminal"]);
    assert!(
        !keys.rejects_pointer(1, false, false),
        "a frame painted at the live generation is immediately current"
    );
    keys.sync_owner(vec!["Workspace", "Prefix"]);
    assert!(
        keys.rejects_pointer(1, false, false),
        "an owner transition rejects the older painted generation"
    );
    assert!(keys.rejects_pointer(2, true, true));
    assert!(!keys.rejects_pointer(2, false, true));
}

#[test]
fn pointer_gate_paints_after_every_overlay_layer() {
    assert!(POINTER_GATE_PRIORITY > fleet_ui_kit::OverlayLayer::Toast.priority());
}

#[test]
fn popup_ctrl_q_guard_is_exact() {
    let mut state = AppState::new("/tmp/fleet", Instant::now());
    state.toggle_agent_popup(Agent::Claude, None);
    let quit = Keystroke::parse("ctrl-q").unwrap_or_else(|error| panic!("{error}"));
    assert!(popup_owns_ctrl_q(&state, &quit));
    let stop = Keystroke::parse("ctrl-shift-q").unwrap_or_else(|error| panic!("{error}"));
    assert!(!popup_owns_ctrl_q(&state, &stop));
    state.open_overlay(Overlay::Dialog(Dialogs::Help));
    assert!(
        !popup_owns_ctrl_q(&state, &quit),
        "a topmost overlay restores normal global ctrl-q handling"
    );
    state.close_overlay();
    assert!(popup_owns_ctrl_q(&state, &quit));
    state.hide_agent_popup();
    assert!(!popup_owns_ctrl_q(&state, &quit));
}

#[test]
fn import_is_rejected_for_a_stale_first_run_context_when_state_exists() {
    assert!(first_run_import_allowed(true, false));
    assert!(!first_run_import_allowed(true, true));
    assert!(!first_run_import_allowed(false, false));
    assert!(!first_run_import_allowed(false, true));
}

struct RootInputFixture {
    state: Entity<AppState>,
    body_focus: FocusHandle,
    board_filter_focus: FocusHandle,
    hub_filter_focus: FocusHandle,
    focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
    visual: VisualTestContext,
}

/// The one repository the filter fixture's worktrees belong to.
fn filter_repo() -> Repo {
    let id: RepoId = "acme/api".parse().unwrap();
    Repo {
        owner: id.owner().to_owned(),
        name: id.name().to_owned(),
        id,
        url: "https://github.com/acme/api.git".to_owned(),
        context_id: "work".parse().unwrap(),
        default_branch: "main".to_owned(),
        path: "/tmp/acme/api".to_owned(),
        cloned_at: "2026-09-19T09:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    }
}

/// One row of the list §3.10's filter narrows. The fixture publishes four: three that `feat`
/// matches, so `ctrl-n` and `down` have somewhere to go, and one that it excludes.
fn filter_worktree(slug: &str) -> Worktree {
    Worktree {
        id: format!("acme/api#{slug}").parse().unwrap(),
        repo_id: "acme/api".parse().unwrap(),
        slug: slug.to_owned(),
        branch: slug.to_owned(),
        base_ref: "main".to_owned(),
        path: format!("/tmp/{slug}"),
        session: format!("acme/api#{slug}"),
        host: None,
        created_at: "2026-09-19T12:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    }
}

fn board_context() -> FleetContext {
    FleetContext {
        id: "work".parse().unwrap(),
        name: "Fleet".to_owned(),
        owners: Vec::new(),
        created_at: "2026-09-19T12:00:00Z".to_owned(),
    }
}

fn board_view(context: &FleetContext) -> BoardView {
    let mut board = new_board(context, "2026-09-19T12:00:00Z");
    let card = create_card(
        &mut board,
        &[],
        "card-1".parse().unwrap(),
        CardDraft {
            title: "Fix login".to_owned(),
            ..CardDraft::default()
        },
        "2026-09-19T12:00:00Z",
    )
    .unwrap();
    BoardView {
        board,
        cards: vec![card],
        live_runs: Vec::new(),
    }
}

fn board_snapshot(context: FleetContext) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-19T12:00:00Z".to_owned(),
        revision: None,
        contexts: vec![context.clone()],
        repos: vec![filter_repo()],
        clones: Vec::new(),
        worktrees: vec![
            filter_worktree("feat-one"),
            filter_worktree("feat-two"),
            filter_worktree("feat-three"),
            filter_worktree("chore-lint"),
        ],
        active_context: Some(context.id),
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: DaemonInfo {
            version: "test".to_owned(),
            pid: 1,
            started_at: "2026-09-19T12:00:00Z".to_owned(),
            home: "/tmp/fleet-shell-input-focus".to_owned(),
        },
    }
}

fn root_input_fixture(cx: &mut gpui::TestAppContext, name: &str) -> RootInputFixture {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let context = board_context();
    let view = board_view(&context);
    let snapshot = board_snapshot(context);
    let mut state = None;
    let mut body_focus = None;
    let mut board_filter_focus = None;
    let mut hub_filter_focus = None;
    let mut focus_owner_keys = None;
    let home = format!("/tmp/fleet-shell-input-focus-{name}");
    let window = cx.add_window(|window, cx| {
        let mut shell = Shell::with_bridge(home.into(), crate::bridge::Bridge::closed(), cx);
        shell.state.update(cx, |app, _| {
            app.apply_snapshot(snapshot, Instant::now());
            app.apply_board_view(view);
            app.daemon = DaemonLink::Connected;
            app.screen = Screen::Hub { tab: HubTab::Board };
            let selected_status = app.board().unwrap().cards[0].status_id.clone();
            app.board.focus.column = app
                .board()
                .unwrap()
                .board
                .statuses
                .iter()
                .position(|status| status.id == selected_status)
                .unwrap();
        });
        state = Some(shell.state.clone());
        body_focus = Some(shell.body_focus.clone());
        board_filter_focus = Some(shell.board.filter_focus_handle(cx));
        hub_filter_focus = Some(shell.hub.filter_focus_handle(cx));
        focus_owner_keys = Some(Rc::clone(&shell.focus_owner_keys));
        shell.observe_window(window, cx);
        shell
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    // gpui derives its focus events from the *rendered* frame and reports an inactive window
    // as having no focus path at all, so an unactivated test window emits neither `Focused`
    // nor `Blurred`. A window a user is typing and clicking in is active.
    visual.update(|window, _| window.activate_window());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    let focus_owner_keys = focus_owner_keys.unwrap();
    finish_test_render(&focus_owner_keys);
    RootInputFixture {
        state: state.unwrap(),
        body_focus: body_focus.unwrap(),
        board_filter_focus: board_filter_focus.unwrap(),
        hub_filter_focus: hub_filter_focus.unwrap(),
        focus_owner_keys,
        visual,
    }
}

/// The same real shell, showing the worktrees list §3.10's filter narrows.
fn root_hub_fixture(cx: &mut gpui::TestAppContext, name: &str) -> RootInputFixture {
    let mut fixture = root_input_fixture(cx, name);
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            app.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            app.hub_pane = HubPane::List;
            cx.notify();
        });
    });
    fixture.visual.run_until_parked();
    fixture
        .visual
        .update(|window, cx| window.draw(cx).clear(cx));
    fixture.visual.run_until_parked();
    finish_test_render(&fixture.focus_owner_keys);
    fixture
}

/// Draws the frame the last input produced, the way `dispatch_root_key` does for a key.
fn settle(fixture: &mut RootInputFixture) {
    fixture.visual.run_until_parked();
    fixture
        .visual
        .update(|window, cx| window.draw(cx).clear(cx));
    fixture.visual.run_until_parked();
    finish_test_render(&fixture.focus_owner_keys);
}

fn hub_filter_query(fixture: &mut RootInputFixture) -> String {
    fixture
        .state
        .read_with(&fixture.visual, |app, _| app.filter.query.clone())
}

fn finish_test_render(keys: &Rc<RefCell<FocusOwnerKeys>>) {
    let generation = keys.borrow().live_generation;
    assert!(keys.borrow_mut().finish_render(generation).is_empty());
}

fn dispatch_root_key(fixture: &mut RootInputFixture, key: &str) {
    let keystroke = Keystroke::parse(key).unwrap();
    fixture.visual.update(|window, cx| {
        window.dispatch_keystroke(keystroke, cx);
    });
    fixture.visual.run_until_parked();
    fixture
        .visual
        .update(|window, cx| window.draw(cx).clear(cx));
    fixture.visual.run_until_parked();
    finish_test_render(&fixture.focus_owner_keys);
}

/// A control's click: the action goes to the focused element, then the frame is painted.
fn dispatch_root_action(fixture: &mut RootInputFixture, action: impl Action) {
    fixture.visual.dispatch_action(action);
    fixture.visual.run_until_parked();
    fixture
        .visual
        .update(|window, cx| window.draw(cx).clear(cx));
    fixture.visual.run_until_parked();
    finish_test_render(&fixture.focus_owner_keys);
}

fn assert_dialog_input_focused(fixture: &mut RootInputFixture) {
    fixture.visual.update(|window, cx| {
        let input = crate::dialogs::focused_input(&fixture.state, cx).expect("dialog input");
        assert!(input.is_focused(window));
    });
}

fn dialog_input_text(fixture: &mut RootInputFixture) -> String {
    fixture
        .visual
        .update(|_, cx| crate::dialogs::focused_input_text(&fixture.state, cx).unwrap())
}

/// Clicks the painted `dialog.field[index]`, the way a pointer reaches a field no key moved to.
///
/// `docs/TESTING-HARNESS.md` §3 is what names the rect, so this is the same address a scenario
/// clicks rather than a guessed pixel.
fn click_dialog_field(fixture: &mut RootInputFixture, index: usize) {
    fleet_ui_kit::harness::set_recording(true);
    fixture
        .visual
        .update(|window, cx| window.draw(cx).clear(cx));
    let name = format!("dialog.field[{index}]");
    let rect = fixture
        .visual
        .update(|window, _| fleet_ui_kit::harness::painted(window))
        .into_iter()
        .rev()
        .find(|target| target.name == name)
        .unwrap_or_else(|| panic!("{name} was not painted"))
        .rect;
    fleet_ui_kit::harness::set_recording(false);
    let position = gpui::point(
        gpui::px(rect.x + rect.w / 2.0),
        gpui::px(rect.y + rect.h / 2.0),
    );
    fixture
        .visual
        .simulate_mouse_down(position, gpui::MouseButton::Left, gpui::Modifiers::none());
    settle(fixture);
}

/// A state change the click had nothing to do with, which runs `reconcile_focus` again.
fn notify_state(fixture: &mut RootInputFixture) {
    let state = fixture.state.clone();
    fixture
        .visual
        .update(|_, cx| state.update(cx, |_, cx| cx.notify()));
    settle(fixture);
}

/// A dialog's field marker must be a *mirror* of focus, not a second opinion about it.
///
/// `dialogs::focused_input` names the editor the shell keeps focused from that marker, and a
/// click focuses an editor without asking the dialog; before `TextInputEvent::Focused` existed
/// the next `AppState` notify pulled the caret back to the field the keyboard had left.
#[track_caller]
fn assert_click_moves_the_field_marker(
    fixture: &mut RootInputFixture,
    index: usize,
    typed: &str,
    expected: &str,
) {
    let clicked = fixture
        .visual
        .update(|_, cx| crate::dialogs::focused_input(&fixture.state, cx));
    click_dialog_field(fixture, index);
    let after_click = fixture
        .visual
        .update(|_, cx| crate::dialogs::focused_input(&fixture.state, cx));
    assert_ne!(
        clicked, after_click,
        "the click must move the marker to the field it landed on"
    );

    notify_state(fixture);
    assert_dialog_input_focused(fixture);
    fixture.visual.simulate_input(typed);
    settle(fixture);
    assert_eq!(dialog_input_text(fixture), expected);
}

#[gpui::test]
fn real_shell_card_create_description_keeps_focus_after_a_click(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "card-create-click");

    dispatch_root_key(&mut fixture, "c");
    assert_dialog_input_focused(&mut fixture);
    assert_click_moves_the_field_marker(&mut fixture, 1, "a description", "a description");
}

#[gpui::test]
fn real_shell_context_owners_keep_focus_after_a_click(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "context-click");

    dispatch_root_key(&mut fixture, "N");
    assert_dialog_input_focused(&mut fixture);
    assert_click_moves_the_field_marker(&mut fixture, 1, "bukhr", "bukhr");
}

#[gpui::test]
fn real_shell_hook_row_keeps_focus_after_a_click(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "hooks-click");
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            app.open_overlay(Overlay::Dialog(Dialogs::EditHooks));
            cx.notify();
        });
    });
    settle(&mut fixture);
    assert_dialog_input_focused(&mut fixture);

    // The editor opens with one blank prepare row and one blank post-create row.
    assert_eq!(
        fixture
            .visual
            .update(|_, cx| crate::dialogs::hook_row_count(&fixture.state, cx)),
        2
    );
    assert_click_moves_the_field_marker(&mut fixture, 1, "cargo test", "cargo test");
}

#[gpui::test]
fn real_shell_board_filter_keeps_input_focus_and_escape_returns_to_body(
    cx: &mut gpui::TestAppContext,
) {
    let mut fixture = root_input_fixture(cx, "board-filter");

    dispatch_root_key(&mut fixture, "/");
    fixture.visual.update(|window, _| {
        assert!(fixture.board_filter_focus.is_focused(window));
    });
    fixture.visual.simulate_input("acli");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.board.filter.clone()),
        "acli"
    );

    dispatch_root_key(&mut fixture, "escape");
    fixture.visual.update(|window, _| {
        assert!(!fixture.board_filter_focus.is_focused(window));
        assert!(fixture.body_focus.is_focused(window));
    });
}

#[gpui::test]
fn real_shell_card_create_title_accepts_platform_text(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "card-create");

    dispatch_root_key(&mut fixture, "c");
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("New card");
    assert_eq!(dialog_input_text(&mut fixture), "New card");
}

#[gpui::test]
fn real_shell_card_detail_title_accepts_platform_text(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "card-detail");

    dispatch_root_key(&mut fixture, "enter");
    dispatch_root_key(&mut fixture, "i");
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input(" revised");
    assert_eq!(dialog_input_text(&mut fixture), "Fix login revised");
}

#[gpui::test]
fn real_shell_card_picker_query_accepts_platform_text(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "card-picker");

    dispatch_root_key(&mut fixture, "s");
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("doing");
    assert_eq!(dialog_input_text(&mut fixture), "doing");
}

#[gpui::test]
fn real_shell_board_settings_text_row_accepts_platform_text(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "board-settings");

    dispatch_root_key(&mut fixture, ",");
    assert_dialog_input_focused(&mut fixture);
    let before = dialog_input_text(&mut fixture);
    fixture.visual.simulate_input(" revised");
    assert_eq!(dialog_input_text(&mut fixture), format!("{before} revised"));
}

#[gpui::test]
fn real_shell_hub_filter_types_and_still_moves_the_list_cursor(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "hub-filter-cursor");

    dispatch_root_key(&mut fixture, "/");
    fixture.visual.update(|window, _| {
        assert!(fixture.hub_filter_focus.is_focused(window));
    });
    fixture.visual.simulate_input("feat");
    settle(&mut fixture);
    assert_eq!(hub_filter_query(&mut fixture), "feat");
    // Three of the fixture's four worktrees match, so the cursor has somewhere to go.
    assert_eq!(
        fixture.state.read_with(&fixture.visual, |app, _| (
            app.displayed_hub.worktrees.len(),
            app.displayed_hub.worktree_total
        )),
        (3, 4)
    );
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.cursors.worktrees),
        0
    );

    // §3.10: `ctrl-n` and `↓` move the **list** while the editor keeps the keyboard. `↓` is
    // also a `FleetTextInput` row, which a single-line editor declines, so the `Filter` row
    // behind it still fires.
    dispatch_root_key(&mut fixture, "ctrl-n");
    dispatch_root_key(&mut fixture, "down");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.cursors.worktrees),
        2
    );
    assert_eq!(hub_filter_query(&mut fixture), "feat");
    fixture.visual.update(|window, _| {
        assert!(fixture.hub_filter_focus.is_focused(window));
    });
}

#[gpui::test]
fn real_shell_hub_filter_escape_keeps_then_clears_the_query(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "hub-filter-escape");

    dispatch_root_key(&mut fixture, "/");
    fixture.visual.simulate_input("feat");
    assert_eq!(hub_filter_query(&mut fixture), "feat");

    // [A13] stage one: the input closes, the filter stays, focus returns to the body.
    dispatch_root_key(&mut fixture, "escape");
    fixture.visual.update(|window, _| {
        assert!(!fixture.hub_filter_focus.is_focused(window));
        assert!(fixture.body_focus.is_focused(window));
    });
    assert_eq!(hub_filter_query(&mut fixture), "feat");
    assert!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.is_none())
    );

    // Stage two clears it, and the editor follows the state it mirrors.
    dispatch_root_key(&mut fixture, "escape");
    assert_eq!(hub_filter_query(&mut fixture), "");
}

#[gpui::test]
fn real_shell_clear_filter_button_clears_from_either_stage(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "hub-filter-clear");

    // Still typing: one click clears what two `Esc` presses would, and hands the keyboard back.
    dispatch_root_key(&mut fixture, "/");
    fixture.visual.simulate_input("feat");
    dispatch_root_action(&mut fixture, crate::actions::filter::Clear);
    assert_eq!(hub_filter_query(&mut fixture), "");
    fixture.visual.update(|window, _| {
        assert!(fixture.body_focus.is_focused(window));
    });
    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(!app.filter.editing);
        assert!(app.overlay.is_none());
    });

    // After the input was left: the retained query goes too.
    dispatch_root_key(&mut fixture, "/");
    fixture.visual.simulate_input("fix");
    dispatch_root_key(&mut fixture, "escape");
    assert_eq!(hub_filter_query(&mut fixture), "fix");
    dispatch_root_action(&mut fixture, crate::actions::filter::Clear);
    assert_eq!(hub_filter_query(&mut fixture), "");
}

#[gpui::test]
fn real_shell_palette_query_accepts_platform_text(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "palette-query");

    dispatch_root_key(&mut fixture, ":");
    assert!(fixture.state.read_with(&fixture.visual, |app, _| matches!(
        app.overlay,
        Some(Overlay::Palette)
    )));
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("Pull");
    assert_eq!(dialog_input_text(&mut fixture), "Pull");

    dispatch_root_key(&mut fixture, "escape");
    fixture.visual.update(|window, _| {
        assert!(fixture.body_focus.is_focused(window));
    });
    assert!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.is_none())
    );
}

/// The same real shell with the repos pane focused, which is where `n` clones.
fn root_repos_fixture(cx: &mut gpui::TestAppContext, name: &str) -> RootInputFixture {
    let mut fixture = root_hub_fixture(cx, name);
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            app.hub_pane = HubPane::Repos;
            cx.notify();
        });
    });
    settle(&mut fixture);
    fixture
}

/// The key context the shell is publishing right now, innermost word last.
fn live_contexts(fixture: &mut RootInputFixture) -> Vec<&'static str> {
    fixture
        .state
        .read_with(&fixture.visual, |app, _| app.context_chain())
}

#[gpui::test]
fn real_shell_create_worktree_branch_accepts_platform_text(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "create-branch");

    dispatch_root_key(&mut fixture, "n");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::CreateWorktree))
    );
    assert_dialog_input_focused(&mut fixture);
    assert_eq!(live_contexts(&mut fixture), vec!["Dialog", "CreateEditing"]);

    fixture.visual.simulate_input("feat/rut");
    assert_eq!(dialog_input_text(&mut fixture), "feat/rut");
    assert_eq!(
        fixture.visual.update(
            |_, cx| crate::dialogs::read_host(&fixture.state, cx, |host, _| host
                .create
                .branch
                .clone())
        ),
        "feat/rut",
        "the draft mirrors the editor"
    );

    // §3.8.1: `Tab` leaves the branch, which is what gives `←` / `→` back to the host cycler.
    dispatch_root_key(&mut fixture, "tab");
    assert_eq!(live_contexts(&mut fixture), vec!["Dialog", "Create"]);
    fixture.visual.update(|window, cx| {
        assert!(crate::dialogs::focused_input(&fixture.state, cx).is_none());
        assert!(fixture.body_focus.is_focused(window) || !fixture.body_focus.is_focused(window));
    });
}

#[gpui::test]
fn real_shell_clone_query_accepts_platform_text(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_repos_fixture(cx, "clone-query");

    dispatch_root_key(&mut fixture, "n");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::CloneRepo))
    );
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("payroll");
    assert_eq!(dialog_input_text(&mut fixture), "payroll");
    assert_eq!(
        fixture.visual.update(
            |_, cx| crate::dialogs::read_host(&fixture.state, cx, |host, _| host
                .clone
                .query
                .clone())
        ),
        "payroll"
    );
}

#[gpui::test]
fn real_shell_context_dialog_types_into_both_fields(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "context-fields");

    dispatch_root_key(&mut fixture, "N");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::NewContext))
    );
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("Buk HR");
    assert_eq!(dialog_input_text(&mut fixture), "Buk HR");

    dispatch_root_key(&mut fixture, "tab");
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("bukhr");
    assert_eq!(dialog_input_text(&mut fixture), "bukhr");
    let (name, owners) = fixture.visual.update(|_, cx| {
        crate::dialogs::read_host(&fixture.state, cx, |host, _| {
            (host.context.name.clone(), host.context.owner_list())
        })
    });
    assert_eq!(name, "Buk HR");
    assert_eq!(owners, vec!["bukhr".to_owned()]);
}

#[gpui::test]
fn real_shell_repository_hooks_grow_a_row_as_they_are_filled(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_repos_fixture(cx, "hook-rows");

    // Row 0 is the `All` pseudo-repo; `j` lands on the fixture's one repository, which is what
    // `e` edits the hooks of.
    dispatch_root_key(&mut fixture, "j");
    dispatch_root_key(&mut fixture, "e");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::EditHooks))
    );
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("bundle install");
    settle(&mut fixture);
    assert_eq!(dialog_input_text(&mut fixture), "bundle install");
    assert_eq!(
        fixture
            .visual
            .update(|_, cx| crate::dialogs::hook_row_count(&fixture.state, cx)),
        3,
        "the filled prepare row grew a blank one under it"
    );

    dispatch_root_key(&mut fixture, "tab");
    assert_dialog_input_focused(&mut fixture);
    assert_eq!(dialog_input_text(&mut fixture), "");
}

/// The settings dialog with a configuration already loaded: the fixture's bridge is closed, so
/// the daemon's `GetConfig` is refused and the rows are planted directly.
fn open_loaded_settings(fixture: &mut RootInputFixture) {
    dispatch_root_key(fixture, ",");
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        let config = fleet_core::config::default_config("/tmp/fleet");
        crate::dialogs::with_host(&state, cx, |host| {
            host.settings.original = Some(config.clone());
            host.settings.config = Some(config);
            host.settings.config_loading = false;
        });
        crate::dialogs::settings_refresh_rows(&state, cx);
    });
    settle(fixture);
}

#[gpui::test]
fn real_shell_settings_text_row_opens_on_enter_and_then_types(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "settings-text");
    open_loaded_settings(&mut fixture);
    assert_eq!(live_contexts(&mut fixture), vec!["Dialog", "Settings"]);

    // Browsing: `Tab` opens Agents and `j` moves to Claude's terminal command, the first
    // free-text row of §3.8.6.
    dispatch_root_key(&mut fixture, "tab");
    dispatch_root_key(&mut fixture, "j");
    assert_eq!(
        fixture.visual.update(
            |_, cx| crate::dialogs::read_host(&fixture.state, cx, |host, _| host.settings.row)
        ),
        1
    );
    fixture.visual.update(|_, cx| {
        assert!(crate::dialogs::focused_input(&fixture.state, cx).is_none());
    });

    dispatch_root_key(&mut fixture, "enter");
    assert_eq!(
        live_contexts(&mut fixture),
        vec!["Dialog", "SettingsEditing"]
    );
    assert_dialog_input_focused(&mut fixture);

    // `j` is a letter now, not a row move.
    fixture.visual.simulate_input("j-code");
    assert_eq!(dialog_input_text(&mut fixture), "claudej-code");
    assert_eq!(
        fixture.visual.update(
            |_, cx| crate::dialogs::read_host(&fixture.state, cx, |host, _| {
                (
                    host.settings.row,
                    host.settings
                        .config
                        .as_ref()
                        .map(|config| config.agent_commands.claude.clone()),
                )
            })
        ),
        (1, Some("claudej-code".to_owned()))
    );
}

#[gpui::test]
fn real_shell_settings_number_row_drops_every_letter(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "settings-number");
    open_loaded_settings(&mut fixture);

    // Sleep › Grace, §3.8.6's first number row.
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        crate::dialogs::with_host(&state, cx, |host| {
            host.settings.section = 2;
            host.settings.row = 1;
        });
        crate::dialogs::settings_refresh_rows(&state, cx);
    });
    settle(&mut fixture);

    dispatch_root_key(&mut fixture, "enter");
    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("x4j2");
    assert_eq!(dialog_input_text(&mut fixture), "200042");
    assert_eq!(
        fixture.visual.update(
            |_, cx| crate::dialogs::read_host(&fixture.state, cx, |host, _| host
                .settings
                .config
                .as_ref()
                .map(|config| config.sleep.grace_ms))
        ),
        Some(200_042)
    );
}

/// `ctrl-d` belongs to the focused editor, so §3.8.4's context delete is the shift variant:
/// this pins both halves, because a documented key that the deeper input swallows is a dead key.
#[gpui::test]
fn context_dialog_delete_is_the_key_the_editor_does_not_own(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "context-delete-key");
    dispatch_root_key(&mut fixture, "E");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::EditContext))
    );
    assert_dialog_input_focused(&mut fixture);

    // Plain `ctrl-d` is delete-forward in the editor and never reaches the dialog.
    dispatch_root_key(&mut fixture, "ctrl-d");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::EditContext))
    );

    dispatch_root_key(&mut fixture, "ctrl-shift-d");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::Confirm)),
        "the delete is routed through the expanded `Y` confirm"
    );
}

/// The rename prompt is one editor and it owns the keyboard from the moment it opens.
#[gpui::test]
fn real_shell_rename_terminal_prompt_takes_the_keyboard(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_hub_fixture(cx, "rename-terminal");
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            app.open_overlay(Overlay::Dialog(Dialogs::RenameTerminal));
            cx.notify();
        });
    });
    settle(&mut fixture);

    assert_dialog_input_focused(&mut fixture);
    fixture.visual.simulate_input("build");
    assert_eq!(dialog_input_text(&mut fixture), "build");
}

/// The same real shell, showing the Workspace of a worktree session.
///
/// The fixture's daemon advertises no capabilities, which is exactly the daemon `ctrl-s b` has
/// to refuse rather than open an empty tab on.
fn root_workspace_fixture(cx: &mut gpui::TestAppContext, name: &str) -> RootInputFixture {
    let mut fixture = root_input_fixture(cx, name);
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            let worktree = filter_worktree("feat-one");
            let session = fleet_core::sessions::Session {
                id: worktree
                    .session
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                host: None,
                kind: fleet_core::sessions::SessionKind::Worktree(worktree.id.clone()),
                cwd: worktree.path.clone(),
                terminals: Vec::new(),
                active_terminal: None,
                slept_at: None,
                kept_terminals: Vec::new(),
            };
            let mut snapshot = app
                .snapshot
                .clone()
                .unwrap_or_else(|| panic!("the fixture installs a snapshot"));
            snapshot.sessions = vec![session.clone()];
            app.apply_snapshot(snapshot, Instant::now());
            app.screen = Screen::Workspace {
                session: session.id,
            };
            cx.notify();
        });
    });
    settle(&mut fixture);
    fixture
}

fn last_toast(fixture: &mut RootInputFixture) -> Option<String> {
    fixture.state.read_with(&fixture.visual, |app, _| {
        app.toasts
            .last()
            .map(|live| live.toast.text.as_ref().to_owned())
    })
}

/// `ctrl-s b` on a daemon without `board.worktree` says so and opens no tab.
#[gpui::test]
fn real_shell_board_key_refuses_a_daemon_without_worktree_boards(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_workspace_fixture(cx, "board-key");

    dispatch_root_key(&mut fixture, "ctrl-s");
    dispatch_root_key(&mut fixture, "b");

    assert_eq!(
        last_toast(&mut fixture).as_deref(),
        Some(crate::state::WORKTREE_BOARDS_UNSUPPORTED)
    );
    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(
            !matches!(app.board.scope, Some(crate::state::BoardScope::Worktree(_))),
            "a refused scope never moves the mirror: {:?}",
            app.board.scope
        );
    });
}

/// The palette row runs the same action, although the palette is a sibling of the Workspace.
///
/// The listener lives on the shell root precisely for this: dispatched from the palette's own
/// dispatch path, a `Workspace > Prefix` listener mounted on the Workspace would never see it,
/// and the row would be one `Enter` that does nothing.
#[gpui::test]
fn real_shell_board_palette_row_reaches_the_same_handler(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_workspace_fixture(cx, "board-palette");

    // `ctrl-s W` is the Workspace's way into the palette; it seeds the session filter, which
    // `ctrl-u` clears the way a user retyping the query would.
    dispatch_root_key(&mut fixture, "ctrl-s");
    dispatch_root_key(&mut fixture, "W");
    assert_dialog_input_focused(&mut fixture);
    dispatch_root_key(&mut fixture, "ctrl-u");
    fixture.visual.simulate_input("Open the worktree's board");
    settle(&mut fixture);
    assert_eq!(dialog_input_text(&mut fixture), "Open the worktree's board");
    dispatch_root_key(&mut fixture, "enter");

    assert_eq!(
        last_toast(&mut fixture).as_deref(),
        Some(crate::state::WORKTREE_BOARDS_UNSUPPORTED)
    );
}

/// A worktree board with two cards in two columns, the shape the pane is driven with.
fn worktree_board_view(context: &FleetContext, worktree: &Worktree) -> BoardView {
    let mut board =
        fleet_core::board::new_worktree_board(context, worktree, "2026-09-19T12:00:00Z");
    let mut cards = Vec::new();
    for (index, title) in ["Fix login", "Ship the board"].into_iter().enumerate() {
        let card = create_card(
            &mut board,
            &cards,
            format!("wt-card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: title.to_owned(),
                ..CardDraft::default()
            },
            "2026-09-19T12:00:00Z",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    // One card per column, so `l` / `h` have somewhere to go and `j` / `k` have nowhere.
    for (index, card) in cards.iter_mut().enumerate() {
        card.status_id = board.statuses[index].id.clone();
    }
    BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    }
}

/// Stages the worktree session the board pane lives in, on a daemon that serves its board.
///
/// Only what the daemon owns is staged: `ptys` shell tabs, then the session's `fleet://board`
/// tab when `board_tab` says it already has one, and `active` is the tab the snapshot says is
/// selected. Pointing the mirror is the pane's own job, and every caller is about what it does
/// with that.
fn stage_board_session(fixture: &mut RootInputFixture, ptys: usize, board_tab: bool, active: u64) {
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            let worktree = filter_worktree("feat-one");
            let mut terminals: Vec<fleet_core::sessions::Terminal> = (1..=ptys)
                .map(|id| {
                    workspace_terminal(id as u64, "sh", fleet_core::sessions::TerminalKind::Pty)
                })
                .collect();
            if board_tab {
                terminals.push(workspace_terminal(
                    ptys as u64 + 1,
                    fleet_core::config::NATIVE_BOARD,
                    fleet_core::sessions::TerminalKind::Native,
                ));
            }
            let session = fleet_core::sessions::Session {
                id: worktree
                    .session
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                host: None,
                kind: fleet_core::sessions::SessionKind::Worktree(worktree.id.clone()),
                cwd: worktree.path.clone(),
                terminals,
                active_terminal: Some(fleet_core::ids::TerminalId(active)),
                slept_at: None,
                kept_terminals: Vec::new(),
            };
            let mut snapshot = app
                .snapshot
                .clone()
                .unwrap_or_else(|| panic!("the fixture installs a snapshot"));
            snapshot.sessions = vec![session.clone()];
            app.daemon_capabilities
                .insert(fleet_proto::response::BOARD_WORKTREE_CAPABILITY.to_owned());
            app.apply_snapshot(snapshot, Instant::now());
            app.screen = Screen::Workspace {
                session: session.id,
            };
            app.sync_terminal_mode();
            cx.notify();
        });
    });
    settle(fixture);
}

/// The real shell standing in a worktree Workspace whose `fleet://board` tab is selected.
///
/// The pane's own `sync_board_scope` runs on the staged notify and points the mirror at the
/// worktree; only then does a worktree board view pass `apply_board_view`'s scope guard, which
/// is the whole point of the guard. The fixture asserts that it did, then answers the load the
/// way fleetd would, with that worktree's board.
fn root_board_pane_fixture(cx: &mut gpui::TestAppContext, name: &str) -> RootInputFixture {
    let mut fixture = root_input_fixture(cx, name);
    stage_board_session(&mut fixture, 1, true, 2);
    let state = fixture.state.clone();
    let worktree = filter_worktree("feat-one");
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            assert!(
                matches!(&app.board.scope, Some(crate::state::BoardScope::Worktree(id))
                    if id == &worktree.id),
                "activating the board tab enters the worktree scope: {:?}",
                app.board.scope
            );
            app.apply_board_view(worktree_board_view(&board_context(), &worktree));
            cx.notify();
        });
    });
    settle(&mut fixture);
    fixture
}

fn workspace_terminal(
    id: u64,
    command: &str,
    kind: fleet_core::sessions::TerminalKind,
) -> fleet_core::sessions::Terminal {
    fleet_core::sessions::Terminal {
        id: fleet_core::ids::TerminalId(id),
        name: format!("t{id}"),
        command: command.to_owned(),
        cwd: "/tmp".to_owned(),
        shell_pid: None,
        foreground_command: None,
        status: fleet_core::sessions::TerminalStatus::Running,
        title: None,
        keep_alive: Vec::new(),
        has_unseen_output: false,
        agent_attention: None,
        kind,
    }
}

fn chain(fixture: &mut RootInputFixture) -> Vec<&'static str> {
    fixture
        .state
        .read_with(&fixture.visual, |app, _| app.context_chain())
}

fn board_focus(fixture: &mut RootInputFixture) -> (usize, usize) {
    fixture.state.read_with(&fixture.visual, |app, _| {
        (app.board.focus.column, app.board.focus.row)
    })
}

/// Every `Hub > Board` key is bound over the pane, and `ctrl-s` is still the prefix.
#[gpui::test]
fn real_shell_board_pane_binds_the_board_keys_and_keeps_the_prefix(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_board_pane_fixture(cx, "board-pane-keys");
    assert_eq!(chain(&mut fixture), vec!["Workspace", "Native", "Board"]);
    assert_eq!(board_focus(&mut fixture), (0, 0));

    // The bare letters walk the board rather than reaching a PTY that is not there.
    dispatch_root_key(&mut fixture, "l");
    assert_eq!(board_focus(&mut fixture), (1, 0));
    dispatch_root_key(&mut fixture, "j");
    assert_eq!(board_focus(&mut fixture), (1, 0), "one card in this column");
    dispatch_root_key(&mut fixture, "h");
    assert_eq!(board_focus(&mut fixture), (0, 0));
    dispatch_root_key(&mut fixture, "enter");
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.overlay.clone()),
        Some(Overlay::Dialog(Dialogs::CardDetail))
    );
    dispatch_root_key(&mut fixture, "escape");
    assert_eq!(chain(&mut fixture), vec!["Workspace", "Native", "Board"]);

    // `]` asks the daemon to move the card and never writes the move itself. The fixture's
    // bridge is closed, so the refusal in the sticky slot is what proves the key reached
    // `board::MoveNextColumn` rather than falling through to the Workspace behind it.
    dispatch_root_key(&mut fixture, "]");
    fixture.visual.run_until_parked();
    fixture.state.read_with(&fixture.visual, |app, _| {
        let sticky = app
            .sticky_error
            .as_ref()
            .unwrap_or_else(|| panic!("the move was attempted"));
        assert!(sticky.text.contains("bridge is closed"), "{sticky:?}");
    });

    // §3.10: the filter takes the letters, and the board word leaves the chain for it.
    dispatch_root_key(&mut fixture, "/");
    assert_eq!(chain(&mut fixture), vec!["Filter", "BoardFilter"]);
    fixture.visual.update(|window, _| {
        assert!(fixture.board_filter_focus.is_focused(window));
    });
    fixture.visual.simulate_input("ship");
    settle(&mut fixture);
    assert_eq!(
        fixture
            .state
            .read_with(&fixture.visual, |app, _| app.board.filter.clone()),
        "ship"
    );
    dispatch_root_key(&mut fixture, "escape");
    assert_eq!(chain(&mut fixture), vec!["Workspace", "Native", "Board"]);
}

/// Leaving the tab leaves the pane: the prefix still belongs to `Workspace > Native`.
#[gpui::test]
fn real_shell_board_pane_gives_the_prefix_and_the_scope_back(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_board_pane_fixture(cx, "board-pane-prefix");
    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(
            matches!(&app.board.scope, Some(crate::state::BoardScope::Worktree(id))
                if id.as_str() == "acme/api#feat-one"),
            "{:?}",
            app.board.scope
        );
    });

    dispatch_root_key(&mut fixture, "ctrl-s");
    assert_eq!(chain(&mut fixture), vec!["Workspace", "Prefix"]);
    dispatch_root_key(&mut fixture, "1");

    // The selection is optimistic in the mode and authoritative in the snapshot; both say the
    // pane is gone, and the mirror goes back to the Hub's context board with it.
    assert_eq!(chain(&mut fixture), vec!["Workspace", "Terminal"]);
    let session = fixture.state.read_with(&fixture.visual, |app, _| {
        app.active_session()
            .unwrap_or_else(|| panic!("the fixture installs a session"))
            .id
            .clone()
    });
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            let mut snapshot = app
                .snapshot
                .clone()
                .unwrap_or_else(|| panic!("the fixture installs a snapshot"));
            snapshot.sessions[0].active_terminal = Some(fleet_core::ids::TerminalId(1));
            app.apply_snapshot(snapshot, Instant::now());
            cx.notify();
        });
    });
    settle(&mut fixture);
    assert_eq!(session.as_str(), "acme/api#feat-one");
    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(!app.board_pane_is_active());
        assert!(
            matches!(&app.board.scope, Some(crate::state::BoardScope::Context(id))
                if id.as_str() == "work"),
            "the Hub must not inherit a worktree scope: {:?}",
            app.board.scope
        );
    });
}

/// The scope `ctrl-s b` claims survives the very notify the keystroke raised.
///
/// fleetd owns the tab list, so the frames between the key and its snapshot still show the tab
/// the user came from. The pane's `synchronize` runs on every one of them, and a release there
/// would point the mirror back at the context and strand the `EnsureWorktreeBoard` the
/// keystroke started — the board would then load only once the daemon listed the tab.
#[gpui::test]
fn real_shell_board_key_keeps_its_scope_until_its_tab_arrives(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "board-key-pending");
    stage_board_session(&mut fixture, 1, true, 1);
    let before = board_generation(&mut fixture);

    dispatch_root_key(&mut fixture, "ctrl-s");
    dispatch_root_key(&mut fixture, "b");
    settle(&mut fixture);

    let worktree = filter_worktree("feat-one");
    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(
            !app.board_pane_is_active(),
            "the snapshot still shows the tab the user came from"
        );
        assert!(
            matches!(&app.board.scope, Some(crate::state::BoardScope::Worktree(id))
                if id == &worktree.id),
            "the mirror stays where the keystroke pointed it: {:?}",
            app.board.scope
        );
        assert_eq!(
            app.board_generation(),
            before.wrapping_add(1),
            "one invalidation, the keystroke's own: a second one strands its load"
        );
    });

    // And the tab arriving is what turns the wait into a pane, with the same load still owed.
    stage_board_session(&mut fixture, 1, true, 2);
    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(app.board_pane_is_active());
        assert!(
            matches!(&app.board.scope, Some(crate::state::BoardScope::Worktree(id))
                if id == &worktree.id),
            "{:?}",
            app.board.scope
        );
        assert_eq!(app.board_generation(), before.wrapping_add(1));
    });
}

/// A tab create that is refused gives the mirror back, rather than holding it for a dead wait.
///
/// The session is full, so the `fleet://board` tab `ctrl-s b` asks for can never arrive — the
/// same shape as any other refused create — and the scope the keystroke claimed has to go back
/// to the context instead of outliving the tab it was waiting for.
#[gpui::test]
fn real_shell_a_refused_board_tab_returns_the_scope(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "board-key-refused");
    stage_board_session(&mut fixture, crate::state::WORKSPACE_TAB_LIMIT, false, 1);

    dispatch_root_key(&mut fixture, "ctrl-s");
    dispatch_root_key(&mut fixture, "b");
    settle(&mut fixture);

    fixture.state.read_with(&fixture.visual, |app, _| {
        assert_eq!(
            app.toasts
                .last()
                .map(|live| live.toast.text.as_ref().to_owned())
                .as_deref(),
            Some(crate::state::WORKSPACE_TAB_LIMIT_NOTICE),
            "the create was attempted and refused"
        );
        assert!(
            matches!(&app.board.scope, Some(crate::state::BoardScope::Context(id))
                if id.as_str() == "work"),
            "a tab that is never going to arrive may not hold the worktree scope: {:?}",
            app.board.scope
        );
    });
}

/// Leaving the Workspace ends the wait: the tab the reply would select has nowhere to be drawn.
#[gpui::test]
fn real_shell_leaving_the_workspace_ends_a_pending_board_claim(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_input_fixture(cx, "board-key-left");
    stage_board_session(&mut fixture, 1, true, 1);
    dispatch_root_key(&mut fixture, "ctrl-s");
    dispatch_root_key(&mut fixture, "b");

    // The worktrees tab, not the Hub's board: only the Workspace's own release can answer here.
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            app.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            cx.notify();
        });
    });
    settle(&mut fixture);

    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(
            matches!(&app.board.scope, Some(crate::state::BoardScope::Context(id))
                if id.as_str() == "work"),
            "the Hub must not inherit a worktree scope: {:?}",
            app.board.scope
        );
    });
}

fn board_generation(fixture: &mut RootInputFixture) -> u64 {
    fixture
        .state
        .read_with(&fixture.visual, |app, _| app.board_generation())
}

/// `o` on a card linked to the worktree the pane is standing in is a no-op that says so.
#[gpui::test]
fn real_shell_board_pane_refuses_to_reopen_its_own_worktree(cx: &mut gpui::TestAppContext) {
    let mut fixture = root_board_pane_fixture(cx, "board-pane-open");
    let state = fixture.state.clone();
    fixture.visual.update(|_, cx| {
        state.update(cx, |app, cx| {
            let worktree = app.active_worktree().cloned();
            app.board
                .view
                .as_mut()
                .unwrap_or_else(|| panic!("the fixture answers the pane's load"))
                .cards[0]
                .worktree_id = worktree;
            cx.notify();
        });
    });
    settle(&mut fixture);

    dispatch_root_key(&mut fixture, "o");

    assert_eq!(
        last_toast(&mut fixture).as_deref(),
        Some(crate::screens::board::ALREADY_IN_WORKTREE)
    );
    fixture.state.read_with(&fixture.visual, |app, _| {
        assert!(matches!(app.screen, Screen::Workspace { .. }));
    });
}
