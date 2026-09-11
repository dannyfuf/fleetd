//! Turns tests for the Claude adapter.

use super::*;
use fleet_core::agents::ItemId;

/// A steer's supersede `result` keeps the turn open until the real one lands.
#[test]
fn a_steer_supersede_result_keeps_the_turn_open() {
    let mut session = ClaudeSession::default();
    let turn = TurnId::new();
    session
        .begin_turn(turn, ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    session.record_steer(turn);
    let superseded = map::handle(
        &mut session,
        frame(
            r#"{"type":"result","subtype":"success","terminal_reason":"aborted_streaming","queued_turn_count":1,"duration_ms":5}"#,
        ),
    );
    assert_eq!(names(&superseded.events), ["token_usage"]);
    assert_eq!(session.active_turn(), Some(turn), "the turn keeps running");
    let settled = map::handle(
        &mut session,
        frame(
            r#"{"type":"result","subtype":"success","terminal_reason":"completed","duration_ms":9}"#,
        ),
    );
    assert!(names(&settled.events).contains(&"turn_settled"));
    assert!(session.active_turn().is_none());
}

/// The interrupted turn's own `result` is what settles it, as an interruption.
#[test]
fn an_interrupted_turns_result_settles_it_as_an_interruption() {
    let mut session = ClaudeSession::default();
    let turn = TurnId::new();
    session
        .begin_turn(turn, ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    session
        .mark_interrupted(turn)
        .unwrap_or_else(|error| panic!("{error}"));
    let output = map::handle(
        &mut session,
        frame(
            r#"{"type":"result","subtype":"success","terminal_reason":"aborted_tools","duration_ms":7}"#,
        ),
    );
    let AgentEvent::TurnAborted { reason, .. } =
        output.events.last().unwrap_or_else(|| panic!("an abort"))
    else {
        panic!("an interrupted turn aborts: {:?}", names(&output.events));
    };
    assert_eq!(reason, &AbortReason::User);
}
