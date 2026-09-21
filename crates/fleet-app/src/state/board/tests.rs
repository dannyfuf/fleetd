use super::*;
use crate::state::test_support::*;
use fleet_core::{
    board::{CardDraft, create_card, new_board, new_worktree_board},
    model::{Context, Worktree},
};
use fleet_proto::event::BoardChangeReason;

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
    BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    }
}

fn view() -> BoardView {
    view_of(&context("work"))
}

/// A worktree of the active context's repository, as the daemon lists it.
fn worktree(slug: &str) -> Worktree {
    Worktree {
        id: format!("buk/payroll#{slug}")
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id: "buk/payroll"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        slug: slug.to_owned(),
        branch: format!("feat/{slug}"),
        base_ref: "origin/main".to_owned(),
        path: format!("/tmp/{slug}"),
        session: "s1".to_owned(),
        host: None,
        created_at: "2026-09-06T12:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    }
}

/// That worktree's own board, as `EnsureWorktreeBoard` would answer it.
///
/// It names the same context as [`view`] and is a different board all the same, which is the
/// whole point of the scope: only `worktree_id` tells the two apart.
fn worktree_view(worktree: &Worktree) -> BoardView {
    BoardView {
        board: new_worktree_board(&context("work"), worktree, "2026-09-06T12:00:00Z"),
        cards: Vec::new(),
        live_runs: Vec::new(),
    }
}

/// The app in a worktree's Workspace, with `active` as the tab the daemon has selected.
///
/// `2` is the `fleet://board` tab the pane draws, `1` the PTY `ctrl-s b` is pressed from.
fn state_in_workspace(active: u64) -> AppState {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-board-pane-state", now);
    let mut session = session_with("payroll/feat", &[1, 2]);
    session.terminals[1].kind = fleet_core::sessions::TerminalKind::Native;
    session.terminals[1].command = NATIVE_BOARD.to_owned();
    session.active_terminal = Some(TerminalId(active));
    let context = context("work");
    let mut snapshot = snapshot();
    snapshot.active_context = Some(context.id.clone());
    snapshot.contexts = vec![context];
    snapshot.sessions = vec![session.clone()];
    state.apply_snapshot(snapshot, now);
    state.screen = Screen::Workspace {
        session: session.id,
    };
    state
}

/// The app on the board tab, connected to a daemon that serves worktree boards.
fn state_on_board_with_worktree_boards() -> AppState {
    let mut state = state_on_board();
    state
        .daemon_capabilities
        .insert(BOARD_WORKTREE_CAPABILITY.to_owned());
    state
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

#[test]
fn a_view_from_the_other_scope_never_lands_in_the_board_slot() {
    let mut state = state_on_board_with_worktree_boards();
    let worktree = worktree("feat-board");
    assert!(state.enter_worktree_board_scope(worktree.id.clone(), Instant::now()));

    state.apply_board_view(view());
    assert!(
        state.board().is_none(),
        "the active context's board is not this worktree's board"
    );

    state.apply_board_view(worktree_view(&worktree));
    assert_eq!(
        state
            .board()
            .and_then(|view| view.board.worktree_id.clone()),
        Some(worktree.id.clone()),
        "its own board is the one this scope admits"
    );

    assert!(state.enter_context_board_scope());
    state.apply_board_view(worktree_view(&worktree));
    assert!(
        state.board().is_none(),
        "a worktree's board is not the context's board, though both name the same context"
    );
    state.apply_board_view(view());
    assert!(state.board().is_some());
}

#[test]
fn a_scope_switched_away_from_and_back_rejects_the_answer_it_left_behind() {
    let mut state = state_on_board_with_worktree_boards();
    let (context_scope, context_generation) = state
        .begin_board_load()
        .unwrap_or_else(|| panic!("the board tab claims its first load"));

    let worktree = worktree("feat-board");
    assert!(state.enter_worktree_board_scope(worktree.id.clone(), Instant::now()));
    let (worktree_scope, worktree_generation) = state
        .begin_board_load()
        .unwrap_or_else(|| panic!("the worktree scope claims its own load"));

    assert!(state.enter_context_board_scope());
    let (again_scope, again_generation) = state
        .begin_board_load()
        .unwrap_or_else(|| panic!("and the context scope claims one back"));
    assert_eq!(context_scope, again_scope, "A \u{2192} B \u{2192} A");
    assert_ne!(context_generation, again_generation);

    state.finish_board_load(
        &worktree_scope,
        worktree_generation,
        Ok(worktree_view(&worktree)),
    );
    assert!(
        state.board().is_none(),
        "B's answer belongs to a scope this mirror has left"
    );
    assert!(
        state.board.loading,
        "and the load A claimed still owns the slot"
    );

    state.finish_board_load(&context_scope, context_generation, Ok(view()));
    assert!(
        state.board().is_none(),
        "the first A load is as stale as B's: same scope, older generation"
    );

    state.finish_board_load(&again_scope, again_generation, Ok(view()));
    assert!(state.board().is_some());
    assert!(!state.board.loading);
}

#[test]
fn a_board_changed_event_only_makes_the_board_on_screen_stale() {
    let mut state = state_on_board_with_worktree_boards();
    let worktree = worktree("feat-board");
    assert!(state.enter_worktree_board_scope(worktree.id.clone(), Instant::now()));
    let shown = worktree_view(&worktree);
    let shown_id = shown.board.id.clone();
    state.apply_board_view(shown);
    assert!(!state.board_stale);

    state.apply_daemon_event(
        Event::BoardChanged {
            board_id: view().board.id,
            reason: BoardChangeReason::CardChanged,
        },
        Instant::now(),
    );
    assert!(
        !state.board_stale,
        "the context's board is not the board this scope shows"
    );

    state.apply_daemon_event(
        Event::BoardChanged {
            board_id: shown_id,
            reason: BoardChangeReason::CardChanged,
        },
        Instant::now(),
    );
    assert!(state.board_stale, "its own board's change is owed a reload");
}

#[test]
fn a_daemon_without_worktree_boards_refuses_the_scope_and_keeps_the_board_it_shows() {
    let mut state = state_on_board();
    state.apply_board_view(view());

    assert!(!state.enter_worktree_board_scope(worktree("feat-board").id, Instant::now()));
    assert_eq!(
        state.board_scope(),
        Some(BoardScope::Context(context("work").id)),
        "a refused scope is not entered"
    );
    assert!(
        state.board().is_some(),
        "and the board that was showing is still showing"
    );
    assert_eq!(
        state
            .toasts
            .last()
            .unwrap_or_else(|| panic!("no toast"))
            .toast
            .text
            .as_ref(),
        WORKTREE_BOARDS_UNSUPPORTED,
        "the sentence carries its own remedy (\u{a7}2.7)"
    );
}

/// A refusal the board pane can see is drawn in the pane, not only spoken in a toast.
///
/// The pane draws whatever the one mirror holds, and a refused scope means the mirror never
/// holds this tab's board: without the failed shape the tab would draw skeleton columns for a
/// load that can never go out, or the Hub's context board under a worktree's heading.
#[test]
fn a_refused_scope_under_the_board_pane_takes_the_failed_shape() {
    let mut state = state_in_workspace(2);
    assert!(state.board_pane_is_active());
    state.apply_board_view(view());
    assert!(state.board().is_some(), "the Hub loaded its board first");

    assert!(!state.enter_worktree_board_scope(worktree("feat").id, Instant::now()));

    assert_eq!(
        state.board.error.as_deref(),
        Some(WORKTREE_BOARDS_UNSUPPORTED),
        "the pane says what the toast said"
    );
    assert!(
        state.board().is_none(),
        "the mirror is not holding this tab's board and may not draw another one under it"
    );
    assert_eq!(
        state.board_scope(),
        Some(BoardScope::Context(context("work").id)),
        "a refused scope is still not entered"
    );
}

/// `ctrl-s b` from a tab that is not the board's has no pane to draw a failure in.
#[test]
fn a_refusal_no_board_pane_can_draw_stays_a_toast() {
    let mut state = state_in_workspace(1);
    assert!(!state.board_pane_is_active());
    state.apply_board_view(view());

    assert!(!state.enter_worktree_board_scope(worktree("feat").id, Instant::now()));

    assert!(
        state.board.error.is_none(),
        "nothing is drawing the board, and the Hub's next visit would inherit the message"
    );
    assert!(
        state.board().is_some(),
        "and the board that was showing is still showing"
    );
    assert_eq!(
        state
            .toasts
            .last()
            .unwrap_or_else(|| panic!("no toast"))
            .toast
            .text
            .as_ref(),
        WORKTREE_BOARDS_UNSUPPORTED
    );
}
