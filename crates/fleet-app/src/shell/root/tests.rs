use super::{first_run_import_allowed, focus::*};
use crate::{
    actions::prefix,
    dialogs::Dialogs,
    state::{AppState, DaemonLink, DaemonLossReason, Overlay, Screen, TerminalMode},
};
use fleet_core::config::Agent;
use gpui::{Action, KeyDownEvent, Keystroke};
use std::time::Instant;

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
    let (consumed, action) = take_live_prefix_action(&mut state, &bound);
    assert!(consumed);
    assert_eq!(
        action.as_ref().map(|action| action.name()),
        Some(Action::name(&prefix::NewTerminal))
    );
    assert_eq!(state.terminal_mode, TerminalMode::Terminal);

    for (keys, expected) in [
        ("v", Action::name(&prefix::ToggleWatchPane)),
        ("V", Action::name(&prefix::DismissWatch)),
        ("N", Action::name(&prefix::NextWatch)),
        ("P", Action::name(&prefix::PrevWatch)),
    ] {
        state.enter_prefix();
        let keystroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = take_live_prefix_action(&mut state, &keystroke);
        assert!(consumed, "{keys} must consume the live one-shot prefix");
        assert_eq!(action.as_ref().map(|action| action.name()), Some(expected));
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    }

    state.enter_prefix();
    let unbound = Keystroke::parse("d").unwrap_or_else(|error| panic!("{error}"));
    let (consumed, action) = take_live_prefix_action(&mut state, &unbound);
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
    let (consumed, action) = take_live_prefix_action(&mut state, &bound);
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
    let (consumed, action) = take_live_prefix_action(&mut state, &unbound);
    assert!(consumed);
    assert!(action.is_none());
    assert_eq!(
        state.agent_popup.as_ref().map(|popup| popup.mode),
        Some(crate::state::AgentPopupMode::Scroll)
    );
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
        assert!(!take_live_prefix_action(&mut state, &keystroke).0);
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
