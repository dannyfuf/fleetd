//! Routing-column queue tests (FEA-5): rule 0, the queue a full board keeps in a routing column,
//! and the freed slot that empties it.

use super::*;
use crate::board::ops::{attention, queued};

/// A preset board whose one slot is taken by a card already working in In Progress.
fn full_board() -> (Board, Vec<Card>, Card) {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let busy = add(&mut board, &mut cards, "busy", "in-progress");
    working(&mut cards, &busy.id);
    (board, cards, busy)
}

fn status(id: &str) -> StatusId {
    id.parse().expect("a static status slug is valid")
}

#[test]
fn an_unblocked_card_entering_a_routing_column_advances_and_starts() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "ready");

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert_eq!(plan.moved, vec![(card.id.clone(), status("in-progress"))]);
    assert_eq!(plan.starts.len(), 1, "{:?}", plan.starts);
    assert_eq!(plan.starts[0].card, card.id);
    assert_eq!(plan.starts[0].status_id.as_str(), "in-progress");
    assert!(plan.queued.is_empty());
    let moved = find(&cards, &card.id);
    assert_eq!(moved.status_id.as_str(), "in-progress");
    assert!(moved.pending_run.is_none());
    assert_eq!(
        messages(moved, ActivityKind::AutoMoved),
        vec!["Moved to In Progress: nothing blocks it"]
    );
    assert!(
        messages(moved, ActivityKind::Moved).is_empty(),
        "`Moved` is a human's move; `attention` reads it as one"
    );
    assert_eq!(
        messages(moved, ActivityKind::RunStarted),
        vec!["Run started"]
    );
}

#[test]
fn a_card_created_in_a_routing_column_advances() {
    let mut board = chain_board();
    let mut cards = Vec::new();
    // A freshly created card is seeded exactly as the daemon seeds one: it has entered Ready.
    let card = add(&mut board, &mut cards, "fresh", "ready");

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert_eq!(plan.moved, vec![(card.id.clone(), status("done"))]);
    assert!(plan.starts.is_empty(), "Done runs nothing");
    let moved = find(&cards, &card.id);
    assert_eq!(moved.status_id.as_str(), "done");
    assert_eq!(
        messages(moved, ActivityKind::AutoMoved),
        vec!["Moved to Done: nothing blocks it"]
    );
}

#[test]
fn a_blocked_card_entering_a_routing_column_stays() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let blocker = add(&mut board, &mut cards, "spec", "todo");
    let card = add(&mut board, &mut cards, "impl", "ready");
    block(&mut cards, &card.id, &[&blocker.id]);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert_eq!(plan, Plan::default());
    let stayed = find(&cards, &card.id);
    assert_eq!(stayed.status_id.as_str(), "ready");
    assert!(stayed.pending_run.is_none());
    assert!(!queued(stayed));
    assert!(messages(stayed, ActivityKind::AutoMoved).is_empty());
}

#[test]
fn a_full_board_queues_the_card_in_the_routing_column() {
    let (mut board, mut cards, _busy) = full_board();
    let card = add(&mut board, &mut cards, "impl", "ready");

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert!(plan.moved.is_empty(), "{:?}", plan.moved);
    assert!(plan.starts.is_empty(), "{:?}", plan.starts);
    assert_eq!(plan.queued, vec![card.id.clone()]);
    let waiting = find(&cards, &card.id);
    assert_eq!(waiting.status_id.as_str(), "ready");
    let pending = waiting
        .pending_run
        .as_ref()
        .expect("a queued card is owed the run of the column it is bound for");
    assert_eq!(pending.status_id.as_str(), "in-progress");
    assert_eq!(pending.since, NOW);
    assert!(queued(waiting));
    assert!(messages(waiting, ActivityKind::AutoMoved).is_empty());
}

#[test]
fn a_queued_card_keeps_its_place_in_line() {
    let (mut board, mut cards, _busy) = full_board();
    let card = add(&mut board, &mut cards, "impl", "ready");
    evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    let live = LiveIndex::from_runs(&cards);
    let mut in_flight = BTreeSet::new();
    let again = re_evaluate(
        &board,
        &mut cards,
        std::slice::from_ref(&card.id),
        &live,
        &mut in_flight,
        LATER,
    )
    .expect("a second in-memory evaluation");

    assert_eq!(again.queued, vec![card.id.clone()]);
    let waiting = find(&cards, &card.id);
    assert_eq!(waiting.status_id.as_str(), "ready");
    assert_eq!(
        waiting
            .pending_run
            .as_ref()
            .map(|pending| pending.since.as_str()),
        Some(NOW),
        "re-evaluating a queued card does not send it to the back of the line"
    );
}

#[test]
fn a_freed_slot_moves_the_queued_card_and_starts_it() {
    let (mut board, mut cards, busy) = full_board();
    let card = add(&mut board, &mut cards, "impl", "ready");
    evaluate(&board, &mut cards, std::slice::from_ref(&card.id));
    assert!(queued(find(&cards, &card.id)));

    // The busy card's run ends, which is what empties `live`.
    let run = cards
        .iter_mut()
        .find(|other| other.id == busy.id)
        .and_then(|busy| busy.runs.last_mut())
        .expect("the busy card's run");
    run.ended_at = Some(LATER.into());
    run.outcome = Some(RunOutcome::Succeeded);
    let next = next_pending(&board, &cards)
        .map(|card| card.id.clone())
        .expect("the queued card is next in line");
    assert_eq!(next, card.id);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&next));

    assert_eq!(plan.moved, vec![(card.id.clone(), status("in-progress"))]);
    assert_eq!(plan.starts.len(), 1, "{:?}", plan.starts);
    assert_eq!(plan.starts[0].card, card.id);
    let moved = find(&cards, &card.id);
    assert_eq!(moved.status_id.as_str(), "in-progress");
    assert!(!queued(moved), "the move takes the card off the queue");
    assert_eq!(
        messages(moved, ActivityKind::AutoMoved),
        vec!["Moved to In Progress: a run slot freed"]
    );
    assert_eq!(
        messages(moved, ActivityKind::RunStarted),
        vec!["Run started"]
    );
}

#[test]
fn a_queued_card_does_not_raise_attention() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let in_line = add(&mut board, &mut cards, "in-line", "ready");
    let arrived = add(&mut board, &mut cards, "arrived", "in-progress");
    park(&mut cards, &in_line.id, "in-progress", EARLIER);
    park(&mut cards, &arrived.id, "in-progress", EARLIER);

    let in_line = find(&cards, &in_line.id);
    assert!(queued(in_line));
    assert!(
        !attention(in_line, NOW),
        "waiting in line for a slot is the queue working"
    );
    let arrived = find(&cards, &arrived.id);
    assert!(!queued(arrived));
    assert!(
        attention(arrived, NOW),
        "a card parked in its own column still turns amber"
    );
}

#[test]
fn a_dependant_released_onto_a_full_board_waits_in_its_routing_column() {
    let (mut board, mut cards, _busy) = full_board();
    let blocker = add(&mut board, &mut cards, "spec", "done");
    let dependant = add(&mut board, &mut cards, "impl", "ready");
    block(&mut cards, &dependant.id, &[&blocker.id]);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&blocker.id));

    assert!(plan.moved.is_empty(), "{:?}", plan.moved);
    assert!(plan.starts.is_empty(), "{:?}", plan.starts);
    assert_eq!(plan.queued, vec![dependant.id.clone()]);
    let waiting = find(&cards, &dependant.id);
    assert_eq!(waiting.status_id.as_str(), "ready");
    assert_eq!(
        waiting
            .pending_run
            .as_ref()
            .map(|pending| pending.status_id.as_str()),
        Some("in-progress")
    );
    assert!(queued(waiting));
}

/// A queued card that something blocks after it queued is taken out of line, so a freed slot
/// goes to the card behind it instead of starting nothing for ever.
#[test]
fn a_queued_card_that_gets_blocked_does_not_starve_the_next_one() {
    let (mut board, mut cards, busy) = full_board();
    let first = add(&mut board, &mut cards, "first", "ready");
    evaluate(&board, &mut cards, std::slice::from_ref(&first.id));
    let second = add(&mut board, &mut cards, "second", "ready");
    let live = LiveIndex::from_runs(&cards);
    let mut in_flight = BTreeSet::new();
    re_evaluate(
        &board,
        &mut cards,
        std::slice::from_ref(&second.id),
        &live,
        &mut in_flight,
        LATER,
    )
    .expect("the second card queues");
    assert!(queued(find(&cards, &first.id)) && queued(find(&cards, &second.id)));
    let blocker = add(&mut board, &mut cards, "blocker", "todo");
    block(&mut cards, &first.id, &[&blocker.id]);
    ended(&mut cards, &busy.id, RunOutcome::Succeeded);

    assert!(clear_stale_queues(&board, &mut cards));
    assert!(find(&cards, &first.id).pending_run.is_none());
    let next = next_pending(&board, &cards)
        .map(|card| card.id.clone())
        .expect("the second card is next in line");
    assert_eq!(next, second.id);
    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&next));
    assert_eq!(plan.starts.len(), 1, "{:?}", plan);
    assert_eq!(plan.starts[0].card, second.id);
    assert!(
        !clear_stale_queues(&board, &mut cards),
        "nothing else is stale"
    );
}
