use super::actions::{NO_REMOTE, NO_REMOTE_URL, adjacent_status, picker_target};
use super::*;

#[gpui::test]
fn live_board_filter_deletes_words_while_ctrl_n_moves_the_list(cx: &mut gpui::TestAppContext) {
    use std::{cell::Cell, rc::Rc};

    struct FilterInputHarness {
        input: Entity<TextInput>,
        moves: Rc<Cell<usize>>,
    }

    impl gpui::Render for FilterInputHarness {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let moves = self.moves.clone();
            gpui::div().key_context("Filter").child(
                gpui::div()
                    .key_context("BoardFilter")
                    .on_action(move |_: &filter_actions::CursorDown, _, cx| {
                        moves.set(moves.get() + 1);
                        cx.stop_propagation();
                    })
                    .child(self.input.clone()),
            )
        }
    }

    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let screen = cx.update(BoardScreen::new);
    let input = screen.filter_input.clone();
    let moves = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, _| FilterInputHarness {
        input: input.clone(),
        moves: moves.clone(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
    visual.simulate_input("alpha beta");
    visual.simulate_keystrokes("ctrl-w");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "alpha "));
    visual.simulate_keystrokes("ctrl-n");
    assert_eq!(moves.get(), 1);
}
use crate::state::BoardFocus;
use fleet_core::board::{BoardView, CardDraft, create_card, new_board};

fn view() -> BoardView {
    titled(&["Fix login".to_owned(), "Ship the board".to_owned()])
}

/// A board holding one card per title, every card in the first column.
fn titled(titles: &[String]) -> BoardView {
    let context = fleet_core::model::Context {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = new_board(&context, "2026-09-06T12:00:00Z");
    let mut cards = Vec::new();
    for (index, title) in titles.iter().enumerate() {
        let card = create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: title.clone(),
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

fn state() -> AppState {
    let mut state = AppState::new("/tmp/fleet-board-screen", Instant::now());
    let view = view();
    let column = view
        .board
        .statuses
        .iter()
        .position(|status| status.id == view.cards[0].status_id)
        .unwrap_or_else(|| panic!("no column"));
    state.board.view = Some(view);
    state.board.focus = BoardFocus { column, row: 0 };
    state
}

#[test]
fn detail_picker_keeps_its_card_when_refresh_moves_it_away_from_selection() {
    let mut state = state();
    let detail_id = selected_card(&state).unwrap().id.clone();
    state.board.view.as_mut().unwrap().cards[0].status_id = "done".parse().unwrap();
    state.clamp_board_focus();
    let selected = selected_card(&state).map(|card| card.id.clone());
    assert_ne!(selected, Some(detail_id.clone()));
    assert_eq!(
        picker_target(selected, true, Some(&detail_id)),
        Some(detail_id)
    );
}

#[test]
fn the_selection_follows_the_filter_not_the_raw_column() {
    let mut state = state();
    assert_eq!(
        selected_card(&state).map(|card| card.title.clone()),
        Some("Fix login".to_owned())
    );
    state.board.filter = "board".to_owned();
    state.clamp_board_focus();
    assert_eq!(
        selected_card(&state).map(|card| card.title.clone()),
        Some("Ship the board".to_owned()),
        "the first visible card is the selected one"
    );
    state.board.filter = "zzz".to_owned();
    state.clamp_board_focus();
    assert!(selected_card(&state).is_none());
}

#[test]
fn adjacent_columns_stop_at_both_ends() {
    let mut state = state();
    state.board.focus.column = 0;
    assert!(adjacent_status(&state, -1).is_none());
    assert!(adjacent_status(&state, 1).is_some());
    let last = state
        .board()
        .unwrap_or_else(|| panic!("no board"))
        .board
        .statuses
        .len()
        - 1;
    state.board.focus.column = last;
    assert!(adjacent_status(&state, 1).is_none());
}

#[test]
fn a_board_without_a_sync_job_is_not_syncing() {
    let state = state();
    assert!(!syncing(&state));
}

#[test]
fn a_read_only_field_is_named_with_the_backends_own_label() {
    let mut state = state();
    let board = &mut state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .board;
    board.backend.kind = "jira".into();
    board.sync.readonly_fields = vec!["priority".into(), "due_date".into()];
    state.apply_backends(vec![fleet_core::board::BackendDescriptor {
        kind: "jira".into(),
        label: "Jira (acli)".into(),
        capabilities: fleet_core::board::BackendCapabilities::default(),
        settings_schema: Vec::new(),
    }]);
    assert_eq!(
        readonly_message(&state, &PickerKind::Priority).as_deref(),
        Some("Priority is read-only on Jira (acli) boards"),
        "the sentence names the field the user pressed a key for, not the wire key"
    );
    assert_eq!(
        readonly_message(&state, &PickerKind::DueDate).as_deref(),
        Some("Due date is read-only on Jira (acli) boards")
    );
    assert_eq!(readonly_message(&state, &PickerKind::Status), None);
    // The repository is Fleet's own link; no backend has an opinion about it.
    assert_eq!(readonly_message(&state, &PickerKind::Repo), None);
}

#[test]
fn a_local_board_refuses_nothing_however_stale_its_list_is() {
    let mut state = state();
    state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .board
        .sync
        .readonly_fields = vec!["priority".into()];
    assert_eq!(readonly_message(&state, &PickerKind::Priority), None);
}

/// Both `x` surfaces answer with the same sentence, and it is not the same sentence for the
/// two cases: the card detail's used to say "no remote issue" about a linked card whose
/// board simply has no address for its backend, naming the wrong fact and hiding the fix.
#[test]
fn the_refusal_x_answers_with_names_which_of_the_two_facts_it_is() {
    let mut card = state()
        .board
        .view
        .as_ref()
        .unwrap_or_else(|| panic!("no board"))
        .cards[0]
        .clone();
    assert_eq!(no_remote_reason(&card), NO_REMOTE);
    card.remote = Some(fleet_core::board::RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "SP-1".into(),
        url: None,
        version: None,
        remote_updated_at: None,
        synced_at: "now".into(),
    });
    assert_eq!(no_remote_reason(&card), NO_REMOTE_URL);
}

#[test]
fn only_a_link_with_an_address_is_openable() {
    let mut card = state()
        .board
        .view
        .as_ref()
        .unwrap_or_else(|| panic!("no board"))
        .cards[0]
        .clone();
    assert_eq!(remote_url(&card), None);
    let mut link = fleet_core::board::RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "SP-1".into(),
        url: None,
        version: None,
        synced_at: "2026-09-06T12:00:00Z".into(),
        remote_updated_at: None,
    };
    card.remote = Some(link.clone());
    assert_eq!(remote_url(&card), None, "a key is not an address");
    link.url = Some("   ".into());
    card.remote = Some(link.clone());
    assert_eq!(remote_url(&card), None, "and neither is blank text");
    link.url = Some(" https://example.test/browse/SP-1 ".into());
    card.remote = Some(link);
    assert_eq!(
        remote_url(&card).as_deref(),
        Some("https://example.test/browse/SP-1")
    );
}

/// The whole board model is derived once per revision, not once per frame.
///
/// `gpui-performance` rule 3 and `docs/APP-CONTRACTS.md` "render prepares nothing": the
/// grouping, the filter, the per-column sort and every tile string are one derivation behind
/// `BoardState::revision`. Neither the clock nor the cursor is an input to it — and the revision
/// has to be the board's own, because a card the daemon answers with lands through
/// [`AppState::apply_card`] and moves no snapshot.
#[test]
fn the_board_model_is_derived_once_per_revision() {
    let titles: Vec<String> = (0..50).map(|index| format!("Card {index}")).collect();
    let mut state = AppState::new("/tmp/fleet-board-projection", Instant::now());
    state.board.view = Some(titled(&titles));
    let cache = RefCell::default();

    let initial = projection::prepare(&state, &cache, 1_788_523_200);
    assert_eq!(initial.shown, 50);
    assert_eq!(
        initial
            .columns
            .iter()
            .map(|column| column.rows.len())
            .sum::<usize>(),
        50
    );

    // Neither the clock nor the cursor is a projection input: 50 tiles are prepared once.
    for tick in 0..5 {
        state.board.focus.row = tick;
        assert!(Rc::ptr_eq(
            &initial,
            &projection::prepare(&state, &cache, 1_788_523_201 + tick as i64)
        ));
    }

    let mut card = state.board().unwrap_or_else(|| panic!("no board")).cards[0].clone();
    card.title = "Renamed by the daemon".to_owned();
    state.apply_card(card);
    let updated = projection::prepare(&state, &cache, 1_788_523_210);
    assert!(
        !Rc::ptr_eq(&initial, &updated),
        "an applied card is a new revision"
    );
    assert!(
        updated
            .columns
            .iter()
            .flat_map(|column| column.rows.iter())
            .any(|row| row.title == "Renamed by the daemon")
    );

    state.board.filter = "renamed".to_owned();
    assert_eq!(projection::prepare(&state, &cache, 1_788_523_211).shown, 1);
}

/// A live run, on the board's first card, with the delegation record behind it.
///
/// The run is what the card knows; the delegation is what the mirror knows. A card-called
/// `DelegationChanged` is the only notice the board gets between two board responses, which is
/// what the test below is about.
fn live_run(state: &mut AppState) -> fleet_core::agents::Delegation {
    use fleet_core::agents::{AgentKind, Delegation, DelegationCaller, DelegationId, ThreadId};

    let view = state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"));
    let (id, child) = (DelegationId::new(), ThreadId::new());
    let card = &mut view.cards[0];
    card.runs.push(fleet_core::board::CardRun {
        id,
        thread_id: Some(child),
        status_id: card.status_id.clone(),
        action: fleet_core::board::ActionKind::Prompt,
        provider: AgentKind::Codex,
        model: None,
        effort: None,
        started_at: "2026-09-20T11:00:00Z".to_owned(),
        ended_at: None,
        outcome: None,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    });
    Delegation {
        id,
        caller: DelegationCaller::Card {
            board: view.board.id.clone(),
            card: view.cards[0].id.clone(),
        },
        caller_turn: None,
        caller_item: None,
        child,
        provider: AgentKind::Codex,
        depth: 1,
        brief: "implement the card".to_owned(),
        expectation: "the tests pass".to_owned(),
        eager: false,
        status: fleet_core::agents::DelegationStatus::Running,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: fleet_core::agents::DeliveryState::Pending,
        created: chrono::DateTime::UNIX_EPOCH,
        finished: None,
        headline: None,
        usage: None,
    }
}

/// The run mark the model carries for the card titled `title`.
fn mark_of(model: &BoardModel, title: &str) -> Option<fleet_ui_kit::RunMark> {
    model
        .columns
        .iter()
        .flat_map(|column| column.rows.iter())
        .find(|row| row.title == title)
        .unwrap_or_else(|| panic!("no row titled {title}"))
        .run
}

/// A child narrating itself is not a new board model.
///
/// `gpui-performance` rule 3, now for the marks: a live child emits `DelegationChanged` once
/// per tool call, and the whole model — every column, every tile string — sits behind
/// `CardMarks::revision` precisely so that a headline nobody draws rebuilds nothing. A status
/// that *is* drawn must still rebuild, or the tile would keep the mark the card has left.
#[test]
fn the_board_model_is_not_rebuilt_when_a_live_childs_headline_changes() {
    let mut state = state();
    state.screen = Screen::Hub { tab: HubTab::Board };
    let mut delegation = live_run(&mut state);
    state.apply_daemon_event(
        fleet_proto::event::Event::DelegationChanged(delegation.clone()),
        Instant::now(),
    );
    let cache = RefCell::default();

    let initial = projection::prepare(&state, &cache, 1_788_523_200);
    assert_eq!(
        mark_of(&initial, "Fix login"),
        Some(fleet_ui_kit::RunMark::Working),
        "the fold ran in the update path and the row carries its answer"
    );

    delegation.headline = Some("reading the reducer".to_owned());
    state.apply_daemon_event(
        fleet_proto::event::Event::DelegationChanged(delegation.clone()),
        Instant::now(),
    );
    assert!(
        Rc::ptr_eq(
            &initial,
            &projection::prepare(&state, &cache, 1_788_523_201)
        ),
        "a headline moves no mark, so the projection key is equal and the model is reused"
    );

    delegation.status = fleet_core::agents::DelegationStatus::Blocked;
    state.apply_daemon_event(
        fleet_proto::event::Event::DelegationChanged(delegation),
        Instant::now(),
    );
    let updated = projection::prepare(&state, &cache, 1_788_523_202);
    assert!(
        !Rc::ptr_eq(&initial, &updated),
        "a child that stopped for a person is a different board"
    );
    assert_eq!(
        mark_of(&updated, "Fix login"),
        Some(fleet_ui_kit::RunMark::NeedsYou)
    );
}

/// A column's list is told what changed rather than rebuilt.
///
/// `gpui-performance` rule 5: the `ListState` carries the measured height of every tile, so a
/// filter that hides seven of eight cards splices the span that moved and leaves the rest alone.
/// A list left at the old count draws rows the model no longer has.
#[gpui::test]
fn a_columns_list_follows_the_row_count_it_is_given(cx: &mut gpui::TestAppContext) {
    let titles: Vec<String> = (0..8).map(|index| format!("Card {index}")).collect();
    let mut state = AppState::new("/tmp/fleet-board-lists", Instant::now());
    state.board.view = Some(titled(&titles));
    let cache = RefCell::default();
    let mut screen = cx.update(BoardScreen::new);

    let model = projection::prepare(&state, &cache, 1_788_523_200);
    screen.sync_lists(&model);
    assert_eq!(screen.column_lists.len(), model.columns.len());
    let rows: usize = screen
        .column_lists
        .iter()
        .map(gpui::ListState::item_count)
        .sum();
    // Every column's list also holds its `Add card` row, after its last tile.
    let footers = model.columns.len();
    assert_eq!(rows, 8 + footers);

    state.board.filter = "Card 1".to_owned();
    let filtered = projection::prepare(&state, &cache, 1_788_523_201);
    screen.sync_lists(&filtered);
    let rows: usize = screen
        .column_lists
        .iter()
        .map(gpui::ListState::item_count)
        .sum();
    assert_eq!(
        rows,
        1 + footers,
        "the lists follow the filter, not the raw board"
    );
}

/// A card edited in the background leaves the column's scroll where the user put it.
///
/// `gpui-performance` rule 5 and `docs/DESIGN-SYSTEM.md` "background events must never re-sort,
/// re-scroll or re-focus": `ListState::splice` moves the scroll anchor to the start of the
/// spliced range whenever the range contains it, which is what `reset` does — so a column
/// spliced whole (`0..len`) on every row change scrolls back to the top and re-measures every
/// tile the moment the daemon answers with one edited card.
#[gpui::test]
fn an_edited_card_leaves_the_column_where_the_user_scrolled_it(cx: &mut gpui::TestAppContext) {
    let mut state = fifty_cards("/tmp/fleet-board-scroll");
    let cache = RefCell::default();
    let mut screen = cx.update(BoardScreen::new);
    let model = projection::prepare(&state, &cache, 1_788_523_200);
    screen.sync_lists(&model);
    let column = full_column(&model);
    scroll_to_row(&screen.column_lists[column], 20);

    let mut card = state.board().unwrap_or_else(|| panic!("no board")).cards[0].clone();
    card.title = "Renamed by the daemon".to_owned();
    state.apply_card(card);
    let updated = projection::prepare(&state, &cache, 1_788_523_201);
    screen.sync_lists(&updated);

    assert_eq!(screen.column_lists[column].item_count(), 50 + 1);
    assert_eq!(
        screen.column_lists[column].logical_scroll_top().item_ix,
        20,
        "one edited row is no reason to scroll the column back to the top"
    );
}

/// A card arriving above the viewport moves the anchor by one row, not to the top.
///
/// The narrowed splice is only worth its comparison if it really is narrow: the fifty tiles
/// below the arrival keep their measured heights, which the list reports by keeping the anchor
/// on the same card — one row further down, because one row arrived above it.
#[gpui::test]
fn a_new_card_shifts_the_anchor_by_the_row_that_arrived(cx: &mut gpui::TestAppContext) {
    let mut state = fifty_cards("/tmp/fleet-board-insert");
    let cache = RefCell::default();
    let mut screen = cx.update(BoardScreen::new);
    let model = projection::prepare(&state, &cache, 1_788_523_200);
    screen.sync_lists(&model);
    let column = full_column(&model);
    scroll_to_row(&screen.column_lists[column], 20);

    let mut arrival = state.board().unwrap_or_else(|| panic!("no board")).cards[0].clone();
    arrival.id = "card-arrival"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    arrival.number = 500;
    arrival.title = "Arrived second".to_owned();
    // Cards are positioned 0, 10, 20, …: 5 lands between the first two.
    arrival.position = 5;
    state.apply_card(arrival);
    let updated = projection::prepare(&state, &cache, 1_788_523_201);
    screen.sync_lists(&updated);

    assert_eq!(screen.column_lists[column].item_count(), 51 + 1);
    assert_eq!(
        screen.column_lists[column].logical_scroll_top().item_ix,
        21,
        "the anchor follows its own card past the row that arrived above it"
    );
}

/// Editing the card the viewport is anchored on keeps the offset inside it.
///
/// A same-length change is [`gpui::ListState::remeasure_items`], not a splice: splicing the
/// anchor's own row keeps its index but zeroes `offset_in_item`, so a title edited two rows up
/// would still jolt the column by the part of the tile the user had scrolled past.
#[gpui::test]
fn editing_the_anchor_row_keeps_the_offset_inside_it(cx: &mut gpui::TestAppContext) {
    let mut state = fifty_cards("/tmp/fleet-board-anchor");
    let cache = RefCell::default();
    let mut screen = cx.update(BoardScreen::new);
    let model = projection::prepare(&state, &cache, 1_788_523_200);
    screen.sync_lists(&model);
    let column = full_column(&model);
    screen.column_lists[column].scroll_to(gpui::ListOffset {
        item_ix: 20,
        offset_in_item: gpui::px(7.0),
    });

    let mut card = state.board().unwrap_or_else(|| panic!("no board")).cards[20].clone();
    card.title = "Renamed under the viewport".to_owned();
    state.apply_card(card);
    let updated = projection::prepare(&state, &cache, 1_788_523_201);
    screen.sync_lists(&updated);

    let top = screen.column_lists[column].logical_scroll_top();
    assert_eq!(top.item_ix, 20);
    assert_eq!(
        top.offset_in_item,
        gpui::px(7.0),
        "the row was remeasured, not replaced"
    );
}

/// A board of fifty cards, all of them in one column.
fn fifty_cards(root: &str) -> AppState {
    let titles: Vec<String> = (0..50).map(|index| format!("Card {index}")).collect();
    let mut state = AppState::new(root, Instant::now());
    state.board.view = Some(titled(&titles));
    state
}

/// The index of the column holding every card.
fn full_column(model: &BoardModel) -> usize {
    model
        .columns
        .iter()
        .position(|column| column.rows.len() >= 50)
        .unwrap_or_else(|| panic!("no full column"))
}

/// Puts the column's viewport on `row`, as a user scrolling down to it would.
fn scroll_to_row(list: &ListState, row: usize) {
    list.scroll_to(gpui::ListOffset {
        item_ix: row,
        offset_in_item: gpui::px(0.0),
    });
    assert_eq!(list.logical_scroll_top().item_ix, row);
}
