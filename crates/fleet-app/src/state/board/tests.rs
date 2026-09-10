use super::*;
use crate::state::test_support::*;
use fleet_core::{
    board::{CardDraft, create_card, new_board},
    model::Context,
};

fn context(id: &str) -> Context {
    Context {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: Vec::new(),
        created_at: "2026-09-06T12:00:00Z".into(),
    }
}

/// A two-card board in the contract's first column, as `EnsureBoard` would answer it.
fn view_of(context: &Context) -> BoardView {
    let mut board = new_board(context, "2026-09-06T12:00:00Z");
    let mut cards = Vec::new();
    for (index, title) in ["Fix login", "Ship the board"].iter().enumerate() {
        let card = create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: (*title).to_owned(),
                ..CardDraft::default()
            },
            "2026-09-06T12:00:00Z",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    BoardView { board, cards }
}

fn view() -> BoardView {
    view_of(&context("work"))
}

/// The app on the board tab of a context the daemon has activated, with nothing loaded yet.
fn state_on_board() -> AppState {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-board-state", now);
    let active = context("work");
    let mut snapshot = snapshot();
    snapshot.active_context = Some(active.id.clone());
    snapshot.contexts = vec![active];
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Hub { tab: HubTab::Board };
    state
}

/// The column a freshly created card lands in, which is the one the focus starts on.
fn first_column(state: &AppState) -> usize {
    let view = state.board().unwrap_or_else(|| panic!("no board"));
    view.board
        .statuses
        .iter()
        .position(|status| status.id == view.cards[0].status_id)
        .unwrap_or_else(|| panic!("no column for the first card"))
}

#[test]
fn a_board_view_for_another_context_never_replaces_the_active_one() {
    let mut state = state_on_board();
    // A context switch bumps the generation but a reply already in flight still arrives, and
    // the board slot belongs to whoever is active now.
    state.apply_board_view(view_of(&context("personal")));
    assert!(
        state.board().is_none(),
        "another context's board is not this context's board"
    );
    assert!(state.board_stale, "and the load it answered is still owed");

    state.apply_board_view(view());
    assert!(state.board().is_some());
    assert!(!state.board_stale);
    assert_eq!(state.board.error, None);
}

#[test]
fn an_applied_card_is_upserted_into_its_column_in_position_order() {
    let mut state = state_on_board();
    state.apply_board_view(view());

    let mut edited = state.board().unwrap_or_else(|| panic!("no board")).cards[0].clone();
    edited.title = "Fix login properly".to_owned();
    state.apply_card(edited);
    let cards = &state.board().unwrap_or_else(|| panic!("no board")).cards;
    assert_eq!(
        cards.len(),
        2,
        "an edit updates a card, it does not add one"
    );
    assert_eq!(cards[0].title, "Fix login properly");

    let mut inserted = cards[1].clone();
    inserted.id = "card-2".parse().unwrap_or_else(|error| panic!("{error}"));
    inserted.title = "Polish".to_owned();
    // The daemon renumbers positions on every move, so a card can come back ahead of the ones
    // already held; the list has to be re-sorted rather than appended to. Two cards can share
    // a position mid-renumber, and the comparator breaks that tie on `created_at`.
    inserted.position = 0;
    inserted.created_at = "2026-09-05T12:00:00Z".to_owned();
    state.apply_card(inserted);
    let titles: Vec<&str> = state
        .board()
        .unwrap_or_else(|| panic!("no board"))
        .cards
        .iter()
        .map(|card| card.title.as_str())
        .collect();
    assert_eq!(titles, ["Polish", "Fix login properly", "Ship the board"]);
}

#[test]
fn a_card_whose_status_this_board_has_no_column_for_only_marks_it_stale() {
    let mut state = state_on_board();
    state.apply_board_view(view());
    let mut orphan = state.board().unwrap_or_else(|| panic!("no board")).cards[0].clone();
    orphan.status_id = "triage".parse().unwrap_or_else(|error| panic!("{error}"));

    state.apply_card(orphan);
    assert!(
        state.board_stale,
        "only a fresh EnsureBoard can say which column this card belongs in now"
    );
    assert_eq!(
        state.board().unwrap_or_else(|| panic!("no board")).cards[0].status_id,
        view().cards[0].status_id,
        "the card is left where the last authoritative view put it"
    );
}

#[test]
fn clearing_the_board_closes_its_dialogs_and_strands_the_load_in_flight() {
    let mut state = state_on_board();
    state.open_overlay(Overlay::Dialog(Dialogs::CardDetail));
    let (requested, generation) = state
        .begin_board_load()
        .unwrap_or_else(|| panic!("the board tab claims its first load"));

    state.clear_board();
    assert_eq!(
        state.overlay, None,
        "a card dialog cannot outlive the board whose card it edits"
    );
    assert!(state.board_stale, "the cleared slot owes a fresh load");

    state.finish_board_load(&requested, generation, Ok(view()));
    assert!(
        state.board().is_none(),
        "the reply belongs to the generation that was cleared"
    );
}

#[test]
fn a_load_never_goes_out_without_a_context_or_a_daemon_and_says_which() {
    let mut cold = AppState::new("/tmp/fleet-board-state", Instant::now());
    cold.screen = Screen::Hub { tab: HubTab::Board };
    assert!(cold.begin_board_load().is_none());
    assert_eq!(
        cold.board.error.as_deref(),
        Some(NO_ACTIVE_CONTEXT),
        "the sentence names the keys that fix it"
    );

    let mut lost = state_on_board();
    lost.daemon = DaemonLink::Lost {
        attempt: 1,
        dismissed: false,
        reason: DaemonLossReason::ConnectionLost,
    };
    assert!(lost.begin_board_load().is_none());
    assert_eq!(lost.board.error.as_deref(), Some("fleetd is not reachable"));

    let mut elsewhere = state_on_board();
    elsewhere.screen = Screen::hub();
    assert!(elsewhere.begin_board_load().is_none());
    assert_eq!(
        elsewhere.board.error, None,
        "a screen that shows no board owes no explanation"
    );
}

#[test]
fn a_superseded_load_never_writes_its_answer_into_the_board() {
    let mut state = state_on_board();
    let (first_context, first) = state
        .begin_board_load()
        .unwrap_or_else(|| panic!("the board tab claims its first load"));
    state.clear_board();
    let (second_context, second) = state
        .begin_board_load()
        .unwrap_or_else(|| panic!("the cleared slot claims another load"));
    assert_ne!(first, second);

    state.finish_board_load(&first_context, first, Err("connection reset".to_owned()));
    assert!(
        state.board.loading,
        "the newer load still owns the slot it claimed"
    );
    assert_eq!(
        state.board.error, None,
        "an abandoned load's failure is not this load's failure"
    );

    state.finish_board_load(&second_context, second, Ok(view()));
    assert!(!state.board.loading);
    assert!(state.board().is_some());
}

#[test]
fn the_focus_is_clamped_to_the_cards_the_filter_leaves_visible() {
    let mut state = state_on_board();
    state.apply_board_view(view());
    let column = first_column(&state);
    state.board.focus = BoardFocus { column, row: 1 };
    state.clamp_board_focus();
    assert_eq!(
        state.board.focus.row, 1,
        "both cards are visible unfiltered"
    );

    state.board.filter = "login".to_owned();
    state.clamp_board_focus();
    assert_eq!(
        state.board.focus.row, 0,
        "the second row addresses a card the filter hides"
    );

    let last = state
        .board()
        .unwrap_or_else(|| panic!("no board"))
        .board
        .statuses
        .len()
        - 1;
    state.board.focus.column = last + 5;
    state.clamp_board_focus();
    assert_eq!(state.board.focus.column, last);
    assert_eq!(state.board.focus.row, 0, "that column shows no card at all");
}

#[test]
fn escape_leaves_the_filter_input_before_it_clears_the_filter() {
    let mut state = state_on_board();
    state.apply_board_view(view());
    assert!(
        !state.board_filter_escape(),
        "with no input and no filter, Esc belongs to whoever is behind the board"
    );

    state.board.filter_editing = true;
    state.board.filter = "login".to_owned();
    assert!(state.board_filter_escape());
    assert!(!state.board.filter_editing);
    assert_eq!(
        state.board.filter, "login",
        "stage one only gives the keyboard back (\u{a7}3.10, [D-15])"
    );

    assert!(state.board_filter_escape());
    assert!(state.board.filter.is_empty());
    assert!(!state.board_filter_escape());
}

#[test]
fn the_filter_input_owns_the_keys_only_while_the_board_is_the_topmost_surface() {
    let mut state = state_on_board();
    state.apply_board_view(view());
    assert!(!state.board_filter_owns_keys(), "the input is not open");

    state.board.filter_editing = true;
    assert!(state.board_filter_owns_keys());

    state.open_overlay(Overlay::Dialog(Dialogs::CardDetail));
    assert!(
        !state.board_filter_owns_keys(),
        "a dialog shadows the board's own key context"
    );
    state.close_overlay();

    state.screen = Screen::hub();
    assert!(
        !state.board_filter_owns_keys(),
        "the worktrees list has its own filter"
    );
}
