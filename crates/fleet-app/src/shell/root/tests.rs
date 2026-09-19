use super::{Shell, first_run_import_allowed, focus::*};
use crate::{
    actions::{fleet::OpenJobs, native_agent, prefix},
    dialogs::Dialogs,
    state::{AppState, DaemonLink, DaemonLossReason, HubTab, Overlay, Screen, TerminalMode},
};
use fleet_core::{
    board::{BoardView, CardDraft, create_card, new_board},
    config::Agent,
    model::Context as FleetContext,
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
    focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
    visual: VisualTestContext,
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
    }
}

fn board_snapshot(context: FleetContext) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-19T12:00:00Z".to_owned(),
        revision: None,
        contexts: vec![context.clone()],
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
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
    let mut focus_owner_keys = None;
    let home = format!("/tmp/fleet-shell-input-focus-{name}");
    let window = cx.add_window(|window, cx| {
        let mut shell = Shell::new(home.into(), cx);
        shell.bridge.shutdown();
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
        board_filter_focus = Some(shell.hub.board_filter_focus_handle(cx));
        focus_owner_keys = Some(Rc::clone(&shell.focus_owner_keys));
        shell.observe_window(window, cx);
        shell
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    let focus_owner_keys = focus_owner_keys.unwrap();
    finish_test_render(&focus_owner_keys);
    RootInputFixture {
        state: state.unwrap(),
        body_focus: body_focus.unwrap(),
        board_filter_focus: board_filter_focus.unwrap(),
        focus_owner_keys,
        visual,
    }
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
