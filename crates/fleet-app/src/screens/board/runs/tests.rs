use super::*;
use crate::state::BoardFocus;
use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationCaller, DelegationId, DelegationStatus, DeliveryState,
    },
    board::{
        Action, ActionKind, Board, BoardView, CardDraft, CardRun, ColumnAgentPrefs,
        ColumnAutomation, PendingRun, RunOutcome, create_card, new_board,
    },
    ids::ContextId,
    model::Context,
};

const NOW: &str = "2026-09-06T12:00:00Z";

/// A worktree board with two cards in its first column, the board's cursor on the first.
fn fixture() -> AppState {
    let context = Context {
        id: ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: Vec::new(),
        created_at: NOW.to_owned(),
    };
    let mut board = new_board(&context, NOW);
    let cards: Vec<_> = ["Fix login", "Ship the board"]
        .into_iter()
        .enumerate()
        .map(|(index, title)| {
            create_card(
                &mut board,
                &[],
                format!("card-{index}")
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                CardDraft {
                    title: title.to_owned(),
                    ..CardDraft::default()
                },
                NOW,
            )
            .unwrap_or_else(|error| panic!("{error}"))
        })
        .collect();
    let column = board
        .statuses
        .iter()
        .position(|status| status.id == cards[0].status_id)
        .unwrap_or_else(|| panic!("no column"));
    let mut state = AppState::new("/tmp/fleet-board-runs", Instant::now());
    state.board.view = Some(BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    });
    state.board.focus = BoardFocus { column, row: 0 };
    state
}

/// The board's cards, mutable.
fn cards(state: &mut AppState) -> &mut Vec<Card> {
    &mut state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .cards
}

/// A run of `card`, live when `outcome` is `None`.
fn run(card: &Card, outcome: Option<RunOutcome>, thread: Option<ThreadId>) -> CardRun {
    CardRun {
        id: DelegationId::new(),
        thread_id: thread,
        status_id: card.status_id.clone(),
        action: ActionKind::Prompt,
        provider: AgentKind::Codex,
        model: None,
        effort: None,
        started_at: NOW.to_owned(),
        ended_at: outcome.is_some().then(|| "2026-09-06T12:05:00Z".to_owned()),
        outcome,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    }
}

/// Gives the first column an `on_enter` action, which is what `>` asks for.
fn automate(state: &mut AppState) {
    let view = state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"));
    let status_id = view.cards[0].status_id.clone();
    let status = view
        .board
        .statuses
        .iter_mut()
        .find(|status| status.id == status_id)
        .unwrap_or_else(|| panic!("no column"));
    status.automation = Some(ColumnAutomation {
        on_enter: Some(Action {
            kind: ActionKind::Prompt,
            instructions: String::new(),
            expect: String::new(),
            agent: ColumnAgentPrefs::default(),
            env: Vec::new(),
        }),
        ..ColumnAutomation::default()
    });
}

/// The board the fixture built, for the keys a sentence names.
fn board(state: &AppState) -> &Board {
    &state
        .board
        .view
        .as_ref()
        .unwrap_or_else(|| panic!("no board"))
        .board
}

#[test]
fn attach_refuses_a_card_that_never_ran_by_its_key() {
    let state = fixture();
    let key = board(&state).prefix.clone();
    let target = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert_eq!(
        target.to_attach().err(),
        Some(format!("{key}-1 has no run"))
    );
}

#[test]
fn attach_takes_the_live_runs_thread_over_the_newest_finished_one() {
    let mut state = fixture();
    let card = cards(&mut state)[0].clone();
    let live_thread = ThreadId::new();
    let live = run(&card, None, Some(live_thread));
    let finished = run(&card, Some(RunOutcome::Succeeded), Some(ThreadId::new()));
    // The live run is deliberately not the newest entry: it is still the one `A` opens,
    // because it is the one still moving.
    cards(&mut state)[0].runs = vec![live, finished];
    let target = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert_eq!(target.to_attach().ok(), Some(live_thread));
}

/// A card that *has* run says so: the run is on the card and its failure is in the detail, so
/// the sentence sends the reader there rather than claiming nothing ever ran. The CLI's
/// `card attach` refuses the same card in the same words (contracts §5.5).
#[test]
fn attach_refuses_a_run_that_never_reached_a_thread() {
    let mut state = fixture();
    let card = cards(&mut state)[0].clone();
    cards(&mut state)[0].runs = vec![run(&card, Some(RunOutcome::Failed), None)];
    let key = board(&state).prefix.clone();
    let target = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert_eq!(
        target.to_attach().err(),
        Some(format!("{key}-1's runs never reached a thread"))
    );
}

/// The mirror is asked before the card's own `runs`, so `X` and the tile agree.
///
/// Regression (`scenarios/board/workflow-chain.scenario`): a card-called `DelegationChanged`
/// names its board and its card and lands in milliseconds, while the run row waits for the
/// `BoardChanged` reload. In that window the face already draws `working`, and `X` refusing
/// with `{KEY} has no live run` would be the two surfaces disagreeing about one card
/// (contracts §5.5, `BOARD.md` §11.8).
#[test]
fn cancel_takes_a_live_child_the_board_has_not_recorded_yet() {
    let mut state = fixture();
    let card = cards(&mut state)[0].clone();
    let board_id = board(&state).id.clone();
    state.agents.apply_delegation(Delegation {
        id: DelegationId::new(),
        caller: DelegationCaller::Card {
            board: board_id,
            card: card.id.clone(),
        },
        caller_turn: None,
        caller_item: None,
        child: ThreadId::new(),
        provider: AgentKind::Codex,
        depth: 1,
        brief: "implement the card".to_owned(),
        expectation: "the tests pass".to_owned(),
        eager: false,
        status: DelegationStatus::Blocked,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: chrono::DateTime::UNIX_EPOCH,
        finished: None,
        headline: None,
        usage: None,
    });
    assert!(
        cards(&mut state)[0].runs.is_empty(),
        "the board reload that records the run has not landed yet"
    );
    let target = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert_eq!(target.to_cancel().ok(), Some(card.id));
}

#[test]
fn cancel_takes_a_live_run_and_an_owed_one_and_refuses_a_finished_one() {
    let mut state = fixture();
    let card = cards(&mut state)[0].clone();
    let key = board(&state).prefix.clone();

    cards(&mut state)[0].runs = vec![run(
        &card,
        Some(RunOutcome::Succeeded),
        Some(ThreadId::new()),
    )];
    let finished = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert_eq!(
        finished.to_cancel().err(),
        Some(format!("{key}-1 has no live run"))
    );

    cards(&mut state)[0].runs = vec![run(&card, None, Some(ThreadId::new()))];
    let live = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert_eq!(live.to_cancel().ok(), Some(card.id.clone()));

    cards(&mut state)[0].runs.clear();
    cards(&mut state)[0].pending_run = Some(PendingRun {
        status_id: card.status_id.clone(),
        since: NOW.to_owned(),
    });
    let owed = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert_eq!(
        owed.to_cancel().ok(),
        Some(card.id.clone()),
        "an owed run holds a slot, and `X` is what drops it"
    );
}

#[test]
fn run_now_is_refused_by_the_column_that_has_no_action() {
    let mut state = fixture();
    let plain = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    let status_id = cards(&mut state)[0].status_id.clone();
    let column = board(&state)
        .statuses
        .iter()
        .find(|status| status.id == status_id)
        .unwrap_or_else(|| panic!("no column"))
        .name
        .clone();
    assert_eq!(
        plain.to_run().err(),
        Some(format!("{column} has no action")),
        "the column refuses, and names itself"
    );

    automate(&mut state);
    let automated = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    let card = cards(&mut state)[0].id.clone();
    assert_eq!(automated.to_run().ok(), Some(card));
}

#[test]
fn a_column_that_only_advances_a_card_is_not_a_column_the_run_key_can_ask() {
    let mut state = fixture();
    let view = state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"));
    let status_id = view.cards[0].status_id.clone();
    let next = view
        .board
        .statuses
        .iter()
        .find(|status| status.id != status_id)
        .map(|status| status.id.clone())
        .unwrap_or_else(|| panic!("no second column"));
    let status = view
        .board
        .statuses
        .iter_mut()
        .find(|status| status.id == status_id)
        .unwrap_or_else(|| panic!("no column"));
    status.automation = Some(ColumnAutomation {
        on_success: Some(next),
        ..ColumnAutomation::default()
    });
    let target = target(&state, false, None).unwrap_or_else(|| panic!("no target"));
    assert!(
        target.to_run().is_err(),
        "`on_success` moves a card the column already finished with; `>` starts nothing there"
    );
}

#[test]
fn the_detail_keeps_acting_on_its_own_card_after_the_cursor_moved() {
    let mut state = fixture();
    let second = cards(&mut state)[1].id.clone();
    let target = target(&state, true, Some(&second)).unwrap_or_else(|| panic!("no target"));
    let key = board(&state).prefix.clone();
    assert_eq!(
        target.to_attach().err(),
        Some(format!("{key}-2 has no run")),
        "the dialog's card, not the board's cursor"
    );
}
