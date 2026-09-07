use super::*;
use super::{
    chrome::header_status_tone,
    terminal::{copy_reaches_pty, end_mouse_drag, popup_mouse_cell_at},
};
use crate::terminal::{AbsoluteCellPoint, AbsoluteCellSelection, SelectionGranularity};

#[test]
fn creation_input_survives_terminal_discovery_for_the_same_owner() {
    let owner = PendingOwner {
        agent: Agent::Claude,
        generation: 7,
    };
    let mut local = Local::default();
    assert!(local.queue_input(owner, PendingInput::Paste("first".to_owned())));
    assert!(local.queue_input(owner, PendingInput::Paste("second".to_owned())));

    assert_eq!(
        local.take_pending(owner),
        vec![
            PendingInput::Paste("first".to_owned()),
            PendingInput::Paste("second".to_owned())
        ]
    );
    assert!(local.pending.is_empty());
    assert_eq!(local.state.pending_owner, None);
}

#[test]
fn popup_creation_input_obeys_shared_bounds() {
    let owner = PendingOwner {
        agent: Agent::Claude,
        generation: 7,
    };
    let mut local = Local::default();
    for _ in 0..PENDING_INPUT_EVENT_CAP {
        assert!(local.queue_input(owner, PendingInput::Paste("a".to_owned())));
    }
    assert!(!local.queue_input(owner, PendingInput::Paste("rejected".to_owned())));

    let pending = local.take_pending(owner);
    assert_eq!(pending.len(), PENDING_INPUT_EVENT_CAP);
    assert!(matches!(pending.last(), Some(PendingInput::Paste(text)) if text == "a"));
}

#[test]
fn creation_input_is_discarded_on_agent_switch_or_generation_change() {
    let claude = PendingOwner {
        agent: Agent::Claude,
        generation: 7,
    };
    let opencode = PendingOwner {
        agent: Agent::Opencode,
        generation: 7,
    };
    let reconnected = PendingOwner {
        agent: Agent::Claude,
        generation: 8,
    };
    let mut local = Local::default();
    assert!(local.queue_input(claude, PendingInput::Paste("stale".to_owned())));
    assert!(local.queue_input(opencode, PendingInput::Paste("switched".to_owned())));
    assert!(local.take_pending(claude).is_empty());
    assert_eq!(
        local.take_pending(opencode),
        vec![PendingInput::Paste("switched".to_owned())]
    );

    assert!(local.queue_input(claude, PendingInput::Paste("old link".to_owned())));
    assert!(local.queue_input(reconnected, PendingInput::Paste("new link".to_owned())));
    assert!(local.take_pending(claude).is_empty());
    assert_eq!(
        local.take_pending(reconnected),
        vec![PendingInput::Paste("new link".to_owned())]
    );
}

#[test]
fn popup_hit_test_subtracts_padding_once() {
    let grid = crate::state::MirrorGrid::new(4, 4);
    let bounds = gpui::Bounds::new(
        gpui::point(px(108.0), px(208.0)),
        gpui::size(px(40.0), px(80.0)),
    );
    let metrics = fleet_ui_kit::CellMetrics {
        width: px(10.0),
        height: px(20.0),
    };

    let cell = popup_mouse_cell_at(&grid, bounds, gpui::point(px(118.0), px(228.0)), metrics)
        .unwrap_or_else(|| panic!("position should map into the popup grid"));
    assert_eq!(cell.viewport, crate::terminal::CellPoint::new(1, 1));
}

#[test]
fn retained_selection_does_not_consume_later_release() {
    let point = AbsoluteCellPoint::new(3, 1);
    let mut local = Local::default();
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
        assert_eq!(end_mouse_drag(&mut local), None);
    }
}

#[test]
fn scroll_mode_cmd_c_never_reaches_pty() {
    assert!(copy_reaches_pty(Some(AgentPopupMode::Terminal)));
    assert!(!copy_reaches_pty(Some(AgentPopupMode::Scroll)));
    assert!(!copy_reaches_pty(Some(AgentPopupMode::Prefix)));
    assert!(!copy_reaches_pty(None));
}

fn model_with(state: AgentTerminalState, activity: AgentActivity) -> Model {
    Model {
        agent: Agent::Claude,
        session: agent_session_id(Agent::Claude)
            .unwrap_or_else(|error| panic!("valid agent session: {error}")),
        mode: AgentPopupMode::Terminal,
        terminal: None,
        base_terminal: None,
        generation: 1,
        primed: false,
        reachable: true,
        cols: None,
        history_epoch: None,
        alt_screen: false,
        scroll_offset: 0,
        scrollback_len: 0,
        terminal_state: state,
        exit_code: None,
        activity,
    }
}

#[test]
fn header_tones_cover_all_agent_states() {
    assert_eq!(
        header_status_tone(&model_with(
            AgentTerminalState::Unknown,
            AgentActivity::Unknown
        )),
        Tone::Warning
    );
    assert_eq!(
        header_status_tone(&model_with(
            AgentTerminalState::Starting,
            AgentActivity::Unknown
        )),
        Tone::Warning
    );
    assert_eq!(
        header_status_tone(&model_with(
            AgentTerminalState::Running,
            AgentActivity::Idle
        )),
        Tone::Success
    );
    assert_eq!(
        header_status_tone(&model_with(
            AgentTerminalState::Running,
            AgentActivity::Unknown
        )),
        Tone::Success
    );
    assert_eq!(
        header_status_tone(&model_with(
            AgentTerminalState::Running,
            AgentActivity::Working
        )),
        Tone::Warning
    );
    assert_eq!(
        header_status_tone(&model_with(AgentTerminalState::Exited, AgentActivity::Idle)),
        Tone::Danger
    );

    let mut unreachable = model_with(AgentTerminalState::Running, AgentActivity::Idle);
    unreachable.reachable = false;
    assert_eq!(header_status_tone(&unreachable), Tone::Danger);
}
