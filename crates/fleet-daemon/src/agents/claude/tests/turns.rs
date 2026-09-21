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

/// A session in which one Fleet-submitted turn has run and settled, which is the only state a
/// harness-initiated turn can follow.
fn after_one_turn() -> ClaudeSession {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let settled = map::handle(
        &mut session,
        frame(
            r#"{"type":"result","subtype":"success","terminal_reason":"completed","duration_ms":3}"#,
        ),
    );
    assert!(names(&settled.events).contains(&"turn_settled"));
    assert!(session.active_turn().is_none());
    session
}

/// A minimal `system/init`, the frame that is the only harness-initiated turn signal.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"a1b2c3d4-5e6f-4a7b-8c9d-0e1f2a3b4c5d","model":"claude-haiku-4-5-20251001","capabilities":["msg_lifecycle_v1"],"tools":["Bash"],"slash_commands":[],"skills":[]}"#;

/// The bug this file exists for: Claude resumes itself when a background task finishes, and the
/// whole report used to be dropped for naming a turn Fleet never opened.
#[test]
fn a_background_task_finishing_opens_a_turn_and_settles_it() {
    let (session, events) = replay(BACKGROUND_TASK);
    let mapped = names(&events);
    assert!(!mapped.contains(&"unknown"), "{mapped:?}");
    assert_eq!(
        mapped
            .iter()
            .filter(|name| **name == "turn_started")
            .count(),
        2,
        "the second init opens the turn Fleet never submitted: {mapped:?}"
    );
    // Two settlements, which is also the proof that neither `result` fell through to the
    // usage-only tripwire: that path emits `token_usage` and never settles.
    assert_eq!(
        mapped
            .iter()
            .filter(|name| **name == "turn_settled")
            .count(),
        2,
        "{mapped:?}"
    );
    let turns = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnStarted { turn, .. } => Some(*turn),
            _ => None,
        })
        .collect::<Vec<_>>();
    let second = *turns.get(1).unwrap_or_else(|| panic!("a second turn"));
    let settled = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnSettled { turn, outcome, .. } => Some((*turn, outcome.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        settled.last(),
        Some(&(second, TurnOutcome::Completed)),
        "the harness-initiated turn settles as itself"
    );

    // The report the user came for, inside the turn that produced it.
    let (report_position, report_item) = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ContentDelta { item, delta, .. } if delta == "BACKGROUND FINISHED" => {
                Some((index, *item))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the harness-initiated turn's answer: {mapped:?}"));
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ItemStarted { turn, item, kind: ItemKind::AssistantText { .. }, .. }
                if *turn == second && *item == report_item
        )),
        "the answer belongs to the second turn"
    );

    // …preceded by the row that says why the turn exists at all, naming the task.
    let (notice_position, notice_text) = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ItemStarted {
                turn,
                kind: ItemKind::Notice { text },
                ..
            } if *turn == second => Some((index, text.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the explanatory row: {mapped:?}"));
    assert!(
        notice_position < report_position,
        "the explanation comes before the answer"
    );
    assert!(
        notice_text.contains("on its own") && notice_text.contains("exit code 0"),
        "the row carries the notification's own summary: {notice_text}"
    );
    assert!(session.active_turn().is_none());
    assert!(
        session.last_task_notification.is_none(),
        "consumed and reset"
    );
}

/// The first init of a process is the handshake: the real CLI sends it with Fleet's first prompt
/// and a stand-in greets with it at spawn, and neither is a turn the CLI opened for itself.
#[test]
fn the_first_init_of_a_process_is_a_handshake_and_opens_nothing() {
    let mut session = ClaudeSession::default();
    let output = map::handle(&mut session, frame(INIT));
    let mapped = names(&output.events);
    assert_eq!(
        mapped.first().copied(),
        Some("session_configured"),
        "{mapped:?}"
    );
    assert!(!mapped.contains(&"turn_started"), "{mapped:?}");
    assert!(session.active_turn().is_none());
}

/// Two inits before any turn has run are still the handshake: "a turn has run" is the
/// condition, not "this is not the first init", so a process that greets twice at spawn cannot
/// leave a fresh thread stuck on a turn nothing settles.
#[test]
fn a_second_init_before_any_turn_has_run_still_opens_nothing() {
    let mut session = ClaudeSession::default();
    let first = map::handle(&mut session, frame(INIT));
    let second = map::handle(&mut session, frame(INIT));
    assert!(!names(&first.events).contains(&"turn_started"));
    assert!(!names(&second.events).contains(&"turn_started"));
    assert!(session.active_turn().is_none());
    assert!(session.last_turn.is_none());
}

/// The turn a harness-initiated `init` opens is the turn its row belongs to.
#[test]
fn a_harness_turn_names_its_own_explanatory_row_as_the_turns_item() {
    let mut session = after_one_turn();
    let output = map::handle(&mut session, frame(INIT));
    let AgentEvent::TurnStarted { turn, user_item } = output
        .events
        .first()
        .unwrap_or_else(|| panic!("the turn leads the init: {:?}", names(&output.events)))
    else {
        panic!(
            "the turn must be announced first: {:?}",
            names(&output.events)
        );
    };
    assert!(
        matches!(
            output.events.get(1),
            Some(AgentEvent::ItemStarted { turn: item_turn, item, kind: ItemKind::Notice { .. }, .. })
                if item_turn == turn && item == user_item
        ),
        "the row the turn names is the row that explains it: {:?}",
        names(&output.events)
    );
    assert_eq!(session.active_turn(), Some(*turn));
}

/// An init landing inside a turn Fleet submitted is the ordinary per-turn handshake.
#[test]
fn an_init_inside_a_fleet_submitted_turn_opens_nothing() {
    let mut session = ClaudeSession::default();
    let turn = TurnId::new();
    session
        .begin_turn(turn, ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let output = map::handle(&mut session, frame(INIT));
    let mapped = names(&output.events);
    assert!(!mapped.contains(&"turn_started"), "{mapped:?}");
    assert!(
        !output.events.iter().any(|event| matches!(
            event,
            AgentEvent::ItemStarted {
                kind: ItemKind::Notice { .. },
                ..
            }
        )),
        "no explanatory row for a turn the user prompted: {mapped:?}"
    );
    assert_eq!(session.active_turn(), Some(turn), "one turn, not two");
}

/// A second init inside the harness turn it already opened does not open a third.
#[test]
fn a_repeated_init_does_not_open_a_second_harness_turn() {
    let mut session = after_one_turn();
    let first = map::handle(&mut session, frame(INIT));
    let turn = session.active_turn();
    let second = map::handle(&mut session, frame(INIT));
    assert!(names(&first.events).contains(&"turn_started"));
    assert!(!names(&second.events).contains(&"turn_started"));
    assert_eq!(session.active_turn(), turn);
}
