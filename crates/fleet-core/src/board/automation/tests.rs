use super::*;
use crate::{
    agents::{DelegationId, ThreadId},
    board::{
        defaults::{new_board, workflow_preset},
        model::{
            CardAgentPrefs, ColumnAgentPrefs, ColumnAutomation, RunOutcome, Status, StatusCategory,
        },
        ops::{CardDraft, create_card},
    },
    ids::ContextId,
    model::Context,
};

mod brief_review;
mod queue;

const NOW: &str = "2026-09-20T12:00:00Z";
const EARLIER: &str = "2026-09-20T11:00:00Z";
const LATER: &str = "2026-09-20T13:00:00Z";

/// A card carrying only its required wire fields, which is what a defaulted card is.
fn card() -> Card {
    serde_json::from_value(serde_json::json!({
        "id": "card-1",
        "boardId": "work",
        "number": 12,
        "title": "Fix login",
        "statusId": "todo",
        "createdAt": "2026-09-20T12:00:00Z",
        "updatedAt": "2026-09-20T12:00:00Z",
    }))
    .expect("a card built from its required wire fields alone")
}

fn action(kind: ActionKind) -> Action {
    Action {
        kind,
        instructions: String::new(),
        expect: String::new(),
        agent: ColumnAgentPrefs::default(),
        env: Vec::new(),
    }
}

/// The seven-column preset: `ready` advances when unblocked, `in-progress` runs the card and
/// `in-review` runs a skill, which is every rule this engine has in one board.
fn preset_board() -> Board {
    let mut board = board_with(Vec::new());
    board.statuses = workflow_preset();
    board
}

/// A board carrying exactly these columns, for the graphs the preset cannot express.
fn board_with(statuses: Vec<Status>) -> Board {
    let mut board = new_board(
        &Context {
            id: ContextId::try_from("work").expect("a static context slug is valid"),
            name: "Fleet".into(),
            owners: vec![],
            created_at: NOW.into(),
        },
        NOW,
    );
    if !statuses.is_empty() {
        board.statuses = statuses;
    }
    board
}

/// Two columns: one that releases a card when nothing blocks it, and the finished column it
/// releases into. Every dependency cascade needs only these two.
fn chain_board() -> Board {
    board_with(vec![
        Status {
            id: "ready".parse().expect("a static status slug is valid"),
            name: "Ready".into(),
            category: StatusCategory::Unstarted,
            color: None,
            automation: Some(ColumnAutomation {
                on_enter: None,
                on_success: None,
                advance_when_unblocked: Some(
                    "done".parse().expect("a static status slug is valid"),
                ),
            }),
        },
        Status {
            id: "done".parse().expect("a static status slug is valid"),
            name: "Done".into(),
            category: StatusCategory::Completed,
            color: None,
            automation: None,
        },
    ])
}

fn add(board: &mut Board, cards: &mut Vec<Card>, id: &str, status: &str) -> Card {
    let card = create_card(
        board,
        cards,
        id.parse().expect("a static card id is valid"),
        CardDraft {
            title: format!("Card {id}"),
            status_id: Some(status.parse().expect("a static status slug is valid")),
            ..CardDraft::default()
        },
        NOW,
    )
    .expect("a card on a valid board");
    cards.push(card.clone());
    card
}

/// Marks `card` as blocked by each of `blockers`, as a person editing the card would.
fn block(cards: &mut [Card], card: &CardId, blockers: &[&CardId]) {
    let card = cards
        .iter_mut()
        .find(|other| other.id == *card)
        .expect("a card that was just added");
    card.blocked_by = blockers.iter().map(|id| (*id).clone()).collect();
}

/// Gives `card` a run that has not ended, which is what a working card looks like.
fn working(cards: &mut [Card], card: &CardId) {
    let card = cards
        .iter_mut()
        .find(|other| other.id == *card)
        .expect("a card that was just added");
    card.runs.push(CardRun {
        id: DelegationId::new(),
        thread_id: Some(ThreadId::new()),
        status_id: card.status_id.clone(),
        action: ActionKind::Prompt,
        provider: AgentKind::Claude,
        model: None,
        effort: None,
        started_at: NOW.into(),
        ended_at: None,
        outcome: None,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    });
}

/// Gives `card` a run that started in the column it stands in and has already ended, which is
/// what a card a delivery has just written an outcome on looks like.
fn ended(cards: &mut [Card], card: &CardId, outcome: RunOutcome) {
    working(cards, card);
    let run = cards
        .iter_mut()
        .find(|other| other.id == *card)
        .and_then(|card| card.runs.last_mut())
        .expect("the run that was just pushed");
    run.ended_at = Some(LATER.into());
    run.outcome = Some(outcome);
}

fn find<'a>(cards: &'a [Card], card: &CardId) -> &'a Card {
    cards
        .iter()
        .find(|other| other.id == *card)
        .expect("a card that was just added")
}

/// Evaluates with an empty reservation and a live index derived from the cards themselves.
fn evaluate(board: &Board, cards: &mut [Card], seeds: &[CardId]) -> Plan {
    let live = LiveIndex::from_runs(cards);
    let mut in_flight = BTreeSet::new();
    re_evaluate(board, cards, seeds, &live, &mut in_flight, NOW).expect("an in-memory evaluation")
}

/// The same evaluation from seeds that did not enter their column.
fn evaluate_settled(board: &Board, cards: &mut [Card], seeds: &[CardId]) -> Plan {
    let live = LiveIndex::from_runs(cards);
    let mut in_flight = BTreeSet::new();
    re_evaluate_settled(board, cards, seeds, &live, &mut in_flight, NOW)
        .expect("an in-memory evaluation")
}

fn messages(card: &Card, kind: ActivityKind) -> Vec<&str> {
    card.activity
        .iter()
        .filter(|entry| entry.kind == kind)
        .map(|entry| entry.message.as_str())
        .collect()
}

/// The one rule this module already decides: a skill column runs on Claude whatever the card asks
/// for — a move is never refused over a preference — while the card's model and effort still
/// reach the run row.
#[test]
fn a_skill_column_runs_on_claude_and_keeps_the_cards_model_and_effort() {
    let mut card = card();
    card.agent = Some(CardAgentPrefs {
        provider: Some(AgentKind::Codex),
        model: Some("gpt-5".to_owned()),
        effort: Some("high".to_owned()),
    });

    let prefs = resolve_prefs(
        &card,
        &action(ActionKind::Skill {
            name: "deep-review".to_owned(),
            args: String::new(),
        }),
    );

    assert_eq!(prefs.provider, Some(AgentKind::Claude));
    assert_eq!(prefs.model.as_deref(), Some("gpt-5"));
    assert_eq!(prefs.effort.as_deref(), Some("high"));
    assert_eq!(prefs.mode, PermissionMode::FullAccess);
}

#[test]
fn a_card_entering_an_action_column_starts_one_run_and_names_the_agent() {
    let mut board = preset_board();
    board.settings.max_live_runs = Some(2);
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");
    cards[0].agent = Some(CardAgentPrefs {
        provider: Some(AgentKind::Codex),
        model: Some("gpt-5".to_owned()),
        effort: Some("high".to_owned()),
    });

    let live = LiveIndex::from_runs(&cards);
    let mut in_flight = BTreeSet::new();
    let plan = re_evaluate(
        &board,
        &mut cards,
        std::slice::from_ref(&card.id),
        &live,
        &mut in_flight,
        NOW,
    )
    .expect("an in-memory evaluation");

    assert_eq!(plan.starts.len(), 1);
    assert_eq!(plan.starts[0].card, card.id);
    assert_eq!(plan.starts[0].status_id.as_str(), "in-progress");
    assert_eq!(plan.starts[0].action.kind, ActionKind::Prompt);
    assert!(in_flight.contains(&card.id));
    assert!(plan.queued.is_empty());
    assert_eq!(
        messages(find(&cards, &card.id), ActivityKind::RunStarted),
        vec!["Run started · codex · gpt-5 · high"]
    );
    assert!(find(&cards, &card.id).pending_run.is_none());
}

#[test]
fn an_agent_nobody_chose_drops_out_of_the_sentence_with_its_separator() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert_eq!(plan.starts.len(), 1);
    assert_eq!(
        messages(find(&cards, &card.id), ActivityKind::RunStarted),
        vec!["Run started"]
    );
}

#[test]
fn a_column_with_no_action_starts_nothing() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "todo", "todo");

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert_eq!(plan, Plan::default());
    assert!(messages(find(&cards, &card.id), ActivityKind::RunStarted).is_empty());
}

#[test]
fn a_card_named_twice_in_the_seeds_starts_one_run() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");

    let plan = evaluate(&board, &mut cards, &[card.id.clone(), card.id.clone()]);

    assert_eq!(plan.starts.len(), 1);
    assert_eq!(
        messages(find(&cards, &card.id), ActivityKind::RunStarted).len(),
        1
    );
}

#[test]
fn re_entering_a_column_with_a_live_run_adds_no_second_start() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");
    working(&mut cards, &card.id);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert!(plan.starts.is_empty());
    assert!(find(&cards, &card.id).pending_run.is_none());
}

/// The rule a terminal run leans on: a card that ran and stopped where it stands is settled,
/// and a settled seed starts nothing.
///
/// Rule 1 alone cannot tell this card from one a person has just moved in — both are sitting in
/// an action column with no live run — which is why the delivery says which of the two happened
/// by choosing its entry point.
#[test]
fn a_settled_seed_whose_run_ended_in_this_column_starts_nothing() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");
    ended(&mut cards, &card.id, RunOutcome::Failed);

    let plan = evaluate_settled(&board, &mut cards, std::slice::from_ref(&card.id));

    assert!(plan.starts.is_empty(), "{:?}", plan.starts);
    assert!(plan.queued.is_empty(), "{:?}", plan.queued);
    let settled = find(&cards, &card.id);
    assert!(settled.pending_run.is_none());
    assert_eq!(settled.runs.len(), 1);
    assert!(messages(settled, ActivityKind::RunStarted).is_empty());

    // The same board state through the entry door starts the run, which is what a person moving
    // the card back into this column asks for — and is exactly why a delivery must not use it.
    let entered = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));
    assert_eq!(entered.starts.len(), 1);
}

/// Every outcome, including the success a column has nowhere to route: the card stayed put, so
/// none of them starts the column again.
#[test]
fn no_outcome_restarts_a_card_that_ended_where_it_started() {
    for outcome in [
        RunOutcome::Succeeded,
        RunOutcome::Failed,
        RunOutcome::NeedsYou,
        RunOutcome::Incomplete,
        RunOutcome::Cancelled,
    ] {
        let mut board = preset_board();
        let mut cards = Vec::new();
        let card = add(&mut board, &mut cards, "impl", "in-progress");
        ended(&mut cards, &card.id, outcome);

        let plan = evaluate_settled(&board, &mut cards, std::slice::from_ref(&card.id));

        assert!(plan.starts.is_empty(), "{outcome:?} restarted the card");
    }
}

/// The half a settled seed keeps: what it blocks is still released by its ending.
#[test]
fn a_settled_seed_still_advances_the_cards_it_blocks() {
    let mut board = chain_board();
    let mut cards = Vec::new();
    let blocker = add(&mut board, &mut cards, "blocker", "done");
    let dependant = add(&mut board, &mut cards, "dependant", "ready");
    block(&mut cards, &dependant.id, &[&blocker.id]);

    let plan = evaluate_settled(&board, &mut cards, std::slice::from_ref(&blocker.id));

    assert_eq!(plan.moved.len(), 1);
    assert_eq!(find(&cards, &dependant.id).status_id.as_str(), "done");
}

/// A card the cascade moves has entered its new column, settled seed or not, so that column
/// runs: the chain a succeeding run advances is exactly this path.
#[test]
fn a_card_the_cascade_moves_is_started_by_the_column_it_lands_in() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let blocker = add(&mut board, &mut cards, "blocker", "done");
    let dependant = add(&mut board, &mut cards, "dependant", "ready");
    block(&mut cards, &dependant.id, &[&blocker.id]);
    ended(&mut cards, &blocker.id, RunOutcome::Succeeded);

    let plan = evaluate_settled(&board, &mut cards, std::slice::from_ref(&blocker.id));

    assert_eq!(plan.moved.len(), 1, "{:?}", plan.moved);
    assert_eq!(plan.starts.len(), 1, "{:?}", plan.starts);
    assert_eq!(plan.starts[0].card, dependant.id);
    assert_eq!(plan.starts[0].status_id.as_str(), "in-progress");
}

#[test]
fn a_card_already_reserved_by_an_earlier_evaluation_is_not_started_again() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");
    let live = LiveIndex::from_runs(&cards);
    let mut in_flight = BTreeSet::new();

    let first = re_evaluate(
        &board,
        &mut cards,
        std::slice::from_ref(&card.id),
        &live,
        &mut in_flight,
        NOW,
    )
    .expect("an in-memory evaluation");
    let second = re_evaluate(
        &board,
        &mut cards,
        std::slice::from_ref(&card.id),
        &live,
        &mut in_flight,
        NOW,
    )
    .expect("an in-memory evaluation");

    assert_eq!(first.starts.len(), 1);
    assert!(second.starts.is_empty());
    assert!(second.queued.is_empty());
}

#[test]
fn a_full_board_parks_the_card_and_keeps_the_time_it_has_waited_from() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let working_card = add(&mut board, &mut cards, "impl", "in-progress");
    let waiting = add(&mut board, &mut cards, "review", "in-review");
    working(&mut cards, &working_card.id);

    let live = LiveIndex::from_runs(&cards);
    let mut in_flight = BTreeSet::new();
    let plan = re_evaluate(
        &board,
        &mut cards,
        std::slice::from_ref(&waiting.id),
        &live,
        &mut in_flight,
        NOW,
    )
    .expect("an in-memory evaluation");

    assert!(plan.starts.is_empty());
    assert_eq!(plan.queued, vec![waiting.id.clone()]);
    let pending = find(&cards, &waiting.id)
        .pending_run
        .clone()
        .expect("a parked card is owed its column's run");
    assert_eq!(pending.status_id.as_str(), "in-review");
    assert_eq!(pending.since, NOW);
    assert!(messages(find(&cards, &waiting.id), ActivityKind::RunStarted).is_empty());
    assert!(in_flight.is_empty());

    let later = re_evaluate(
        &board,
        &mut cards,
        std::slice::from_ref(&waiting.id),
        &live,
        &mut in_flight,
        LATER,
    )
    .expect("a second in-memory evaluation");

    assert_eq!(later.queued, vec![waiting.id.clone()]);
    assert_eq!(
        find(&cards, &waiting.id)
            .pending_run
            .as_ref()
            .map(|pending| pending.since.as_str()),
        Some(NOW),
        "a card keeps the time it has been waiting from"
    );
}

#[test]
fn an_archived_card_starts_nothing() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");
    cards[0].archived = true;

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&card.id));

    assert_eq!(plan, Plan::default());
}

#[test]
fn a_satisfied_blocker_advances_its_dependant_and_that_column_starts_it() {
    let mut board = preset_board();
    board.settings.max_live_runs = Some(2);
    let mut cards = Vec::new();
    let blocker = add(&mut board, &mut cards, "spec", "done");
    let dependant = add(&mut board, &mut cards, "impl", "ready");
    block(&mut cards, &dependant.id, &[&blocker.id]);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&blocker.id));

    assert_eq!(
        plan.moved,
        vec![(
            dependant.id.clone(),
            "in-progress"
                .parse()
                .expect("a static status slug is valid")
        )]
    );
    assert_eq!(plan.starts.len(), 1);
    assert_eq!(plan.starts[0].card, dependant.id);
    let moved = find(&cards, &dependant.id);
    assert_eq!(moved.status_id.as_str(), "in-progress");
    assert_eq!(
        messages(moved, ActivityKind::AutoMoved),
        vec!["Moved to In Progress: unblocked by FLE-1 reaching Done"]
    );
    assert!(
        messages(moved, ActivityKind::Moved).is_empty(),
        "`Moved` is a human's move; `attention` reads it as one"
    );
}

#[test]
fn a_dependant_still_blocked_by_another_card_stays_where_it_is() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let first = add(&mut board, &mut cards, "spec", "done");
    let second = add(&mut board, &mut cards, "design", "in-progress");
    let dependant = add(&mut board, &mut cards, "impl", "ready");
    block(&mut cards, &dependant.id, &[&first.id, &second.id]);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&first.id));

    assert!(plan.moved.is_empty());
    assert_eq!(find(&cards, &dependant.id).status_id.as_str(), "ready");
}

#[test]
fn a_working_dependant_is_not_advanced_out_of_its_run() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let blocker = add(&mut board, &mut cards, "spec", "done");
    let dependant = add(&mut board, &mut cards, "impl", "ready");
    block(&mut cards, &dependant.id, &[&blocker.id]);
    working(&mut cards, &dependant.id);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&blocker.id));

    assert!(plan.moved.is_empty());
    assert_eq!(find(&cards, &dependant.id).status_id.as_str(), "ready");
}

#[test]
fn a_diamond_moves_its_sink_once() {
    let mut board = chain_board();
    let mut cards = Vec::new();
    let root = add(&mut board, &mut cards, "root", "done");
    let left = add(&mut board, &mut cards, "left", "ready");
    let right = add(&mut board, &mut cards, "right", "ready");
    let sink = add(&mut board, &mut cards, "sink", "ready");
    block(&mut cards, &left.id, &[&root.id]);
    block(&mut cards, &right.id, &[&root.id]);
    block(&mut cards, &sink.id, &[&left.id, &right.id]);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&root.id));

    let done: StatusId = "done".parse().expect("a static status slug is valid");
    assert_eq!(
        plan.moved,
        vec![
            (left.id.clone(), done.clone()),
            (right.id.clone(), done.clone()),
            (sink.id.clone(), done)
        ]
    );
    assert_eq!(
        messages(find(&cards, &sink.id), ActivityKind::AutoMoved),
        vec!["Moved to Done: unblocked by FLE-2 reaching Done"],
        "the sink is reached down both arms and moves once"
    );
}

#[test]
fn a_cycle_nobody_can_satisfy_moves_nothing_and_terminates() {
    let mut board = chain_board();
    let mut cards = Vec::new();
    let root = add(&mut board, &mut cards, "root", "done");
    let first = add(&mut board, &mut cards, "first", "ready");
    let second = add(&mut board, &mut cards, "second", "ready");
    block(&mut cards, &first.id, &[&root.id, &second.id]);
    block(&mut cards, &second.id, &[&root.id, &first.id]);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&root.id));

    assert_eq!(plan, Plan::default());
    assert_eq!(find(&cards, &first.id).status_id.as_str(), "ready");
    assert_eq!(find(&cards, &second.id).status_id.as_str(), "ready");
}

#[test]
fn a_cycle_that_can_advance_visits_each_card_once() {
    let mut board = chain_board();
    let mut cards = Vec::new();
    let root = add(&mut board, &mut cards, "root", "done");
    let first = add(&mut board, &mut cards, "first", "ready");
    let second = add(&mut board, &mut cards, "second", "ready");
    // A cycle no validation would accept, because a document can be edited by hand.
    block(&mut cards, &first.id, &[&root.id, &second.id]);
    block(&mut cards, &second.id, &[&root.id]);

    let plan = evaluate(&board, &mut cards, std::slice::from_ref(&root.id));

    assert_eq!(plan.moved.len(), 2, "each card moves once");
    assert_eq!(find(&cards, &first.id).status_id.as_str(), "done");
    assert_eq!(find(&cards, &second.id).status_id.as_str(), "done");
    assert_eq!(
        messages(find(&cards, &first.id), ActivityKind::AutoMoved).len(),
        1
    );
}

#[test]
fn a_freed_slot_goes_to_the_later_column_and_then_to_the_oldest_wait() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let implementing = add(&mut board, &mut cards, "impl", "in-progress");
    let newest_review = add(&mut board, &mut cards, "review-new", "in-review");
    let oldest_review = add(&mut board, &mut cards, "review-old", "in-review");
    park(&mut cards, &implementing.id, "in-progress", EARLIER);
    park(&mut cards, &newest_review.id, "in-review", LATER);
    park(&mut cards, &oldest_review.id, "in-review", NOW);

    assert_eq!(
        next_pending(&board, &cards).map(|card| card.id.clone()),
        Some(oldest_review.id)
    );
}

#[test]
fn a_pending_run_whose_column_lost_its_action_is_never_handed_a_slot() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");
    park(&mut cards, &card.id, "in-progress", NOW);
    for status in &mut board.statuses {
        if status.id.as_str() == "in-progress" {
            status.automation = None;
        }
    }

    assert!(next_pending(&board, &cards).is_none());
}

#[test]
fn an_archived_card_is_never_handed_a_slot() {
    let mut board = preset_board();
    let mut cards = Vec::new();
    let card = add(&mut board, &mut cards, "impl", "in-progress");
    park(&mut cards, &card.id, "in-progress", NOW);
    cards[0].archived = true;

    assert!(next_pending(&board, &cards).is_none());
}

/// Owes `card` the run of `status`, as a throttled evaluation does.
fn park(cards: &mut [Card], card: &CardId, status: &str, since: &str) {
    let card = cards
        .iter_mut()
        .find(|other| other.id == *card)
        .expect("a card that was just added");
    card.pending_run = Some(PendingRun {
        status_id: status.parse().expect("a static status slug is valid"),
        since: since.into(),
    });
}

#[test]
fn the_brief_renders_the_instructions_the_card_and_nothing_else() {
    let mut card = card();
    card.title = "Fix login".into();
    card.description = "The session cookie expires too early.".into();
    let mut action = action(ActionKind::Prompt);
    action.instructions = "Implement {key} — {title} — in the current worktree.".into();

    let brief = brief(&action, "FLE-12", &card, &[]);

    assert_eq!(
        brief,
        "Implement FLE-12 — Fix login — in the current worktree.\n\n\
         # FLE-12 — Fix login\n\n\
         The session cookie expires too early.\n"
    );
}

#[test]
fn a_skill_action_opens_the_brief_with_its_invocation() {
    let card = card();
    let action = action(ActionKind::Skill {
        name: "deep-review".to_owned(),
        args: "--strict".to_owned(),
    });

    let brief = brief(&action, "FLE-12", &card, &[]);

    assert!(
        brief.starts_with("/deep-review --strict\n\n# FLE-12 — Fix login\n"),
        "{brief}"
    );
}

#[test]
fn previous_reports_reach_the_child_newest_first() {
    let card = card();
    let older = Comment {
        id: "one".into(),
        author: None,
        body: "the first run's report".into(),
        created_at: EARLIER.into(),
        remote_id: None,
        run_id: Some(DelegationId::new()),
    };
    let newer = Comment {
        id: "two".into(),
        author: None,
        body: "the second run's report".into(),
        created_at: LATER.into(),
        remote_id: None,
        run_id: Some(DelegationId::new()),
    };

    let brief = brief(
        &action(ActionKind::Prompt),
        "FLE-12",
        &card,
        &[&older, &newer],
    );

    let reports = brief
        .split_once("## Previous run reports")
        .expect("a brief carrying reports names them")
        .1;
    assert!(
        reports.find("the second run's report") < reports.find("the first run's report"),
        "{reports}"
    );
    assert!(reports.contains(&format!("### Report from {LATER}")));
}
