use super::*;
use crate::state::test_support::*;
use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationCaller, DelegationId, DelegationStatus, DeliveryState,
        ThreadId,
    },
    board::{
        ActionKind, CardDraft, ColumnAutomation, LiveRun, StatusCategory, create_card, new_board,
        new_worktree_board,
    },
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

/// Regression: a board the *daemon* changed leaves the Workspace's pane a load to claim.
///
/// Column automation writes to a worktree board without this app asking — it records a run,
/// carries a card on through `on_success`, releases one whose blockers finished — and
/// [`AppState::apply_daemon_event`] answers the `BoardChanged` that follows by marking the
/// mirror stale and nothing else. The Hub tab consumes that flag through the board screen's
/// own observation; the pane did not, so every run mark, the header counts and an
/// auto-advanced card waited for a hand-typed `r`
/// (`screens::board::lifecycle::synchronize`).
#[test]
fn a_daemon_side_change_leaves_the_worktree_pane_a_load_to_claim() {
    let mut state = state_in_workspace(2);
    state
        .daemon_capabilities
        .insert(BOARD_WORKTREE_CAPABILITY.to_owned());
    let worktree = worktree("feat-board");
    assert!(state.enter_worktree_board_scope(worktree.id.clone(), Instant::now()));
    let shown = worktree_view(&worktree);
    let shown_id = shown.board.id.clone();
    state.apply_board_view(shown);
    assert!(
        state.board_pane_is_active(),
        "the pane is the surface drawing this scope"
    );
    assert!(
        state.begin_board_load().is_none(),
        "a view just applied owes nothing"
    );

    state.apply_daemon_event(
        Event::BoardChanged {
            board_id: shown_id,
            reason: BoardChangeReason::CardChanged,
        },
        Instant::now(),
    );

    assert!(
        state.begin_board_load().is_some(),
        "the pane's observation has a load to claim"
    );
    assert!(
        state.begin_board_load().is_none(),
        "and only one: a burst of daemon events costs a single request"
    );
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

/// A fixed wall clock: `Stalled` and `attention` are decided by the fixture, never by the host.
fn at(stamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(stamp)
        .unwrap_or_else(|error| panic!("{error}"))
        .with_timezone(&Utc)
}

/// The moment every marks test reads as "now".
const NOW: &str = "2026-09-20T12:00:00Z";

/// A run started by the card's own column, live until `outcome` says otherwise.
fn run_on(card: &Card, outcome: Option<RunOutcome>) -> CardRun {
    CardRun {
        id: DelegationId::new(),
        thread_id: Some(ThreadId::new()),
        status_id: card.status_id.clone(),
        action: ActionKind::Prompt,
        provider: AgentKind::Codex,
        model: None,
        effort: None,
        started_at: "2026-09-20T11:00:00Z".to_owned(),
        ended_at: outcome.is_some().then(|| "2026-09-20T11:30:00Z".to_owned()),
        outcome,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
    }
}

/// The delegation record behind a card's run, as `DelegationChanged` carries it.
fn card_delegation(view: &BoardView, run: &CardRun, status: DelegationStatus) -> Delegation {
    Delegation {
        id: run.id,
        caller: DelegationCaller::Card {
            board: view.board.id.clone(),
            card: view.cards[0].id.clone(),
        },
        caller_turn: None,
        caller_item: None,
        child: run
            .thread_id
            .unwrap_or_else(|| panic!("a run that reached a thread")),
        provider: AgentKind::Codex,
        depth: 1,
        brief: "implement the card".to_owned(),
        expectation: "the tests pass".to_owned(),
        eager: false,
        status,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: chrono::DateTime::UNIX_EPOCH,
        finished: None,
        headline: None,
        usage: None,
    }
}

/// The marks of the board's first card, which is the one every fixture below runs.
///
/// A card with neither mark is absent from the map, so "nothing to say" and "not there" are
/// the same answer and the default is the honest one.
fn first_marks(state: &AppState) -> TileMarks {
    let view = state.board().unwrap_or_else(|| panic!("no board"));
    state
        .board
        .marks
        .by_card
        .get(&view.cards[0].id)
        .copied()
        .unwrap_or_default()
}

#[test]
fn a_live_run_marks_its_card_as_working() {
    let mut state = state_on_board();
    let mut view = view();
    let run = run_on(&view.cards[0], None);
    view.live_runs.push(LiveRun {
        card_id: view.cards[0].id.clone(),
        run: run.id,
        status: DelegationStatus::Running,
        headline: None,
        started: run.started_at.clone(),
    });
    view.cards[0].runs.push(run);

    state.apply_board_view(view);

    assert_eq!(
        first_marks(&state).run,
        Some(RunMark::Working),
        "applying the view is what derives the mark; nothing re-derives it in render"
    );
    assert_eq!(
        (
            state.board.marks.working,
            state.board.marks.waiting,
            state.board.marks.needs_you
        ),
        (1, 0, 0),
        "a live run holds a slot, waits for none, and asks nobody for anything"
    );
}

#[test]
fn a_child_that_goes_blocked_turns_its_card_amber_without_a_board_reload() {
    let mut state = state_on_board();
    let mut view = view();
    let run = run_on(&view.cards[0], None);
    let delegation = card_delegation(&view, &run, DelegationStatus::Blocked);
    view.cards[0].runs.push(run);
    state.apply_board_view(view);
    assert_eq!(first_marks(&state).run, Some(RunMark::Working));

    state.apply_daemon_event(Event::DelegationChanged(delegation), Instant::now());

    assert_eq!(
        first_marks(&state).run,
        Some(RunMark::NeedsYou),
        "the mirror is the only notice the board gets that the child stopped for a person"
    );
    assert_eq!(
        state.board.marks.working, 1,
        "a blocked child is still holding the checkout"
    );
    assert_eq!(
        state.board.marks.needs_you, 1,
        "the header counts the card its tile says needs you, though its run has not ended"
    );
}

#[test]
fn a_card_called_child_paints_working_before_the_board_records_its_run() {
    let mut state = state_on_board();
    let view = view();
    let run = run_on(&view.cards[0], None);
    let delegation = card_delegation(&view, &run, DelegationStatus::Running);
    // The view the app holds is the one from *before* the run started: the daemon writes the
    // run onto the card and announces it as a `BoardChanged`, which costs a whole
    // `EnsureWorktreeBoard` round trip, and a five-second child can end before that lands.
    state.apply_board_view(view);
    assert_eq!(first_marks(&state).run, None, "no run on the card yet");

    state.apply_daemon_event(Event::DelegationChanged(delegation), Instant::now());

    assert_eq!(
        first_marks(&state).run,
        Some(RunMark::Working),
        "a card-called delegation names its own card, so the mark needs no board response"
    );
    assert_eq!(
        state.board.marks.working, 1,
        "a child the mirror has and the view has not is still holding a run slot"
    );
}

#[test]
fn a_card_called_child_that_starts_blocked_paints_needs_you_before_the_board_records_it() {
    let mut state = state_on_board();
    let view = view();
    let run = run_on(&view.cards[0], None);
    let delegation = card_delegation(&view, &run, DelegationStatus::Blocked);
    state.apply_board_view(view);

    state.apply_daemon_event(Event::DelegationChanged(delegation), Instant::now());

    assert_eq!(
        first_marks(&state).run,
        Some(RunMark::NeedsYou),
        "a child parked on its gate is the one thing the tile must say without a reload"
    );
    assert_eq!(
        state.board.marks.needs_you, 1,
        "and the header says it with the tile"
    );
}

#[test]
fn a_run_the_card_has_already_finished_outranks_a_mirror_row_that_has_not_caught_up() {
    let mut state = state_on_board();
    let mut view = view();
    let run = run_on(&view.cards[0], Some(RunOutcome::Cancelled));
    let delegation = card_delegation(&view, &run, DelegationStatus::Running);
    view.cards[0].runs.push(run);
    state.apply_board_view(view);

    state.apply_daemon_event(Event::DelegationChanged(delegation), Instant::now());

    assert_eq!(
        first_marks(&state).run,
        None,
        "the card is the authority on a run it has already ended; a stale mirror row is not"
    );
    assert_eq!(state.board.marks.working, 0);
}

#[test]
fn a_headline_only_delegation_change_leaves_the_marks_where_they_were() {
    let mut state = state_on_board();
    let mut view = view();
    let run = run_on(&view.cards[0], None);
    let delegation = card_delegation(&view, &run, DelegationStatus::Running);
    view.cards[0].runs.push(run);
    state.apply_board_view(view);
    state.apply_daemon_event(Event::DelegationChanged(delegation.clone()), Instant::now());
    let revision = state.board.marks.revision;

    let mut talking = delegation;
    talking.headline = Some("reading the reducer".to_owned());
    state.apply_daemon_event(Event::DelegationChanged(talking), Instant::now());

    assert_eq!(
        state.board.marks.revision, revision,
        "a child narrating itself changes no mark, and the projection is keyed on this counter"
    );
}

#[test]
fn an_owed_run_turns_amber_once_the_wait_is_the_story() {
    let mut state = state_on_board();
    let mut view = view();
    view.cards[0].pending_run = Some(PendingRun {
        status_id: view.cards[0].status_id.clone(),
        since: "2026-09-20T11:59:00Z".to_owned(),
    });
    state.apply_board_view(view);

    state.refresh_card_marks(at("2026-09-20T11:59:59Z"));
    assert_eq!(first_marks(&state).run, Some(RunMark::Pending));
    assert_eq!(
        (state.board.marks.working, state.board.marks.waiting),
        (1, 1),
        "an owed run counts against the live limit that is holding it up, and as waiting"
    );

    state.refresh_card_marks(at(NOW));
    assert_eq!(
        first_marks(&state).run,
        Some(RunMark::Stalled),
        "sixty seconds owed is a wait worth noticing"
    );
    assert_eq!(
        state.board.marks.needs_you, 1,
        "and `attention` agrees, so the header says so too"
    );
}

#[test]
fn a_run_that_never_reached_a_thread_raises_the_sticky_slot_once() {
    let mut state = state_on_board();
    state.apply_board_view(view());
    assert!(
        state.sticky_error.is_none(),
        "the first load has no previous board to call a run new against"
    );

    let mut failed = view();
    let mut run = run_on(&failed.cards[0], Some(RunOutcome::Failed));
    run.thread_id = None;
    run.detail = Some("codex is not installed".to_owned());
    failed.cards[0].runs.push(run);
    state.apply_board_view(failed.clone());
    assert_eq!(
        state.sticky_error.as_ref().map(|error| error.text.as_str()),
        Some("codex is not installed"),
        "nothing else on the board can say why the card stopped"
    );

    state.sticky_error = None;
    state.apply_board_view(failed);
    assert!(
        state.sticky_error.is_none(),
        "the same run is not news twice, however often the board reloads"
    );
}

#[test]
fn two_live_blockers_read_as_waiting_and_a_canceled_one_as_stuck() {
    let mut state = state_on_board();
    let mut view = view();
    let third = create_card(
        &mut view.board,
        &view.cards,
        "card-2".parse().unwrap_or_else(|error| panic!("{error}")),
        CardDraft {
            title: "Land it".to_owned(),
            ..CardDraft::default()
        },
        "2026-09-06T12:00:00Z",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    view.cards.push(third);
    let blockers = vec![view.cards[0].id.clone(), view.cards[1].id.clone()];
    view.cards[2].blocked_by = blockers;
    let blocked_id = view.cards[2].id.clone();
    state.apply_board_view(view.clone());

    assert_eq!(
        state
            .board
            .marks
            .by_card
            .get(&blocked_id)
            .and_then(|marks| marks.blocked),
        Some((2, BlockedTone::Muted)),
        "two cards still doing their work are ordinary waiting"
    );

    // A blocker nobody can finish never releases this card on its own: canceling is a decision
    // not to do the work, not a report that it is done.
    let canceled = view
        .board
        .statuses
        .iter()
        .find(|status| status.category == StatusCategory::Canceled)
        .unwrap_or_else(|| panic!("no canceled column"))
        .id
        .clone();
    view.cards[0].status_id = canceled;
    state.apply_board_view(view);
    assert_eq!(
        state
            .board
            .marks
            .by_card
            .get(&blocked_id)
            .and_then(|marks| marks.blocked),
        Some((2, BlockedTone::Warning)),
        "and that is a person's problem, not a queue's"
    );
}

#[test]
fn a_success_is_marked_only_where_the_column_does_not_carry_the_card_on() {
    let mut state = state_on_board();
    let mut view = view();
    let run = run_on(&view.cards[0], Some(RunOutcome::Succeeded));
    let column = run.status_id.clone();
    view.cards[0].runs.push(run);
    state.apply_board_view(view.clone());
    assert_eq!(
        first_marks(&state).run,
        Some(RunMark::Succeeded),
        "the card is still here, and the check is the only thing saying the work is done"
    );

    let next = view.board.statuses[1].id.clone();
    for status in &mut view.board.statuses {
        if status.id == column {
            status.automation = Some(ColumnAutomation {
                on_success: Some(next.clone()),
                ..ColumnAutomation::default()
            });
        }
    }
    state.apply_board_view(view);
    assert_eq!(
        first_marks(&state).run,
        None,
        "a column that advances on success says so by moving the card, not with a second mark"
    );
}

#[test]
fn a_board_with_no_runs_and_no_links_carries_no_marks_at_all() {
    let mut state = state_on_board();
    state.apply_board_view(view());
    let revision = state.board.marks.revision;

    assert!(
        state.board.marks.by_card.is_empty(),
        "a daemon that runs nothing leaves every tile exactly as it was"
    );
    assert_eq!(
        (state.board.marks.working, state.board.marks.needs_you),
        (0, 0)
    );

    state.refresh_card_marks(at(NOW));
    assert_eq!(
        state.board.marks.revision, revision,
        "a tick that changes no mark must not rebuild a whole board's model"
    );
}
