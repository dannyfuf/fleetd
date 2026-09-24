//! Dragging a card: the gesture ends in the request `[` / `]` would send, with a place.
//!
//! The board is rendered for real — columns, virtualized lists, tiles — and driven with the
//! mouse down, motion and up gpui turns into a drag. The recording bridge keeps what the drop
//! asked the daemon for, which is the whole contract: the card, its new status and its index.

use std::{collections::BTreeMap, time::Instant};

use fleet_core::board::{BoardView, CardDraft, create_card, new_board};
use fleet_proto::request::RequestBody;
use gpui::{
    Bounds, FocusHandle, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext,
    Window, point, prelude::*, px,
};

use super::actions::{Destination, drop_destination};
use super::*;
use crate::bridge::RecordedRequests;

/// A board of three columns' worth of statuses: `cards[0]` holds `Card 0..n` in the first
/// column, and `in_third` cards are moved into the third.
fn board(first: usize, in_third: usize) -> BoardView {
    let context = fleet_core::model::Context {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = new_board(&context, "2026-09-06T12:00:00Z");
    let mut cards = Vec::new();
    for index in 0..first + in_third {
        let card = create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: format!("Card {index}"),
                ..CardDraft::default()
            },
            "2026-09-06T12:00:00Z",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    let (first_status, third_status) = (board.statuses[0].id.clone(), board.statuses[2].id.clone());
    for (index, card) in cards.iter_mut().enumerate() {
        let (status, rank) = if index < first {
            (first_status.clone(), index)
        } else {
            (third_status.clone(), index - first)
        };
        card.status_id = status;
        card.position = u64::try_from(rank).unwrap_or_default() * 10;
    }
    BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    }
}

/// The board screen in a window of its own, as the Hub lends it one.
struct DragHarness {
    state: Entity<AppState>,
    bridge: Bridge,
    screen: BoardScreen,
    focus: FocusHandle,
}

impl Render for DragHarness {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let body = self
            .screen
            .render(&self.state, &self.bridge, &self.focus, window, cx);
        fleet_ui_kit::AppFrame::new().body(body)
    }
}

/// Opens the board in a window and returns the state, the recorded requests and the window.
fn open(
    view: BoardView,
    cx: &mut TestAppContext,
) -> (
    Entity<AppState>,
    RecordedRequests,
    Entity<DragHarness>,
    &mut VisualTestContext,
) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet-board-drag", Instant::now());
        state.board.view = Some(view);
        state
    });
    let (bridge, requests) = Bridge::recording();
    let harness_state = state.clone();
    let (harness, visual) = cx.add_window_view(|_, cx| DragHarness {
        state: harness_state,
        bridge,
        screen: BoardScreen::new(cx),
        focus: cx.focus_handle(),
    });
    visual.run_until_parked();
    (state, requests, harness, visual)
}

/// Where the harness painted `name`, read back from the recorded targets.
fn painted(name: &str, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.update(|window, _| {
        fleet_ui_kit::harness::painted(window)
            .into_iter()
            .find(|target| target.name == name)
            .map(|target| {
                Bounds::new(
                    point(px(target.rect.x), px(target.rect.y)),
                    gpui::size(px(target.rect.w), px(target.rect.h)),
                )
            })
            .unwrap_or_else(|| {
                let names: Vec<_> = fleet_ui_kit::harness::painted(window)
                    .into_iter()
                    .map(|target| target.name)
                    .collect();
                panic!("{name} was not painted: {names:?}")
            })
    })
}

/// Fully visible card target bounds, keyed by the target name the harness records.
fn visible_card_bounds(cx: &mut VisualTestContext) -> BTreeMap<String, Bounds<Pixels>> {
    cx.update(|window, _| {
        let viewport = window.bounds();
        fleet_ui_kit::harness::painted(window)
            .into_iter()
            .filter(|target| target.name.starts_with("board.column[0].card["))
            .filter_map(|target| {
                let bounds = Bounds::new(
                    point(px(target.rect.x), px(target.rect.y)),
                    gpui::size(px(target.rect.w), px(target.rect.h)),
                );
                (bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom())
                    .then_some((target.name.to_string(), bounds))
            })
            .collect()
    })
}

/// Presses at `from`, moves in steps to `to`, and releases there.
fn drag(from: Point<Pixels>, to: Point<Pixels>, cx: &mut VisualTestContext) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    for step in 1..=4u8 {
        let fraction = f32::from(step) / 4.0;
        let at = from + (to - from) * fraction;
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
    }
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

/// The `MoveCard` requests among everything the window asked for.
fn moves(requests: &RecordedRequests) -> Vec<RequestBody> {
    requests
        .take()
        .into_iter()
        .filter(|body| matches!(body, RequestBody::MoveCard { .. }))
        .collect()
}

/// Card 0 dropped under the one card of the third column is the request `]` `]` would send,
/// plus the place: index 1, after the card it was dropped below.
#[gpui::test]
fn a_card_dragged_to_another_column_asks_for_that_status_at_that_place(cx: &mut TestAppContext) {
    fleet_ui_kit::harness::set_recording(true);
    let view = board(2, 1);
    let (card, status) = (view.cards[0].id.clone(), view.board.statuses[2].id.clone());
    let (state, requests, harness, visual) = open(view, cx);
    let from = painted("board.column[0].card[0]", visual).center();
    let below = painted("board.column[2].card[0]", visual);
    let to = point(below.center().x, below.bottom() + px(12.0));
    fleet_ui_kit::harness::set_recording(false);

    // Mid-drag, the column under the pointer knows the slot and says what the drop does.
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(
        from + (to - from) * 0.5,
        MouseButton::Left,
        Modifiers::none(),
    );
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    let (key, name) = visual.update(|_, cx| {
        let view = state.read(cx).board().unwrap_or_else(|| panic!("no board"));
        (
            view.cards[0].display_key(&view.board),
            view.board.statuses[2].name.clone(),
        )
    });
    let mid = visual.update(|_, cx| harness.read(cx).screen.drag.borrow().clone());
    assert_eq!(mid.source, Some(card.clone()));
    assert_eq!(
        mid.target,
        Some(crate::views::board_screen::DropTarget {
            column: 2,
            slot: 1,
            label: Some(format!("Drop to move {key} to {name}").into()),
        })
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();

    assert_eq!(
        moves(&requests),
        [RequestBody::MoveCard {
            card_id: card.clone(),
            status_id: status,
            index: Some(1),
            cancel_run: false,
        }]
    );
    visual.update(|_, cx| {
        let app = state.read(cx);
        assert_eq!(
            selected_card(app).map(|selected| selected.id.clone()),
            Some(card),
            "the drop selects the card it moves, as a press on its tile does"
        );
    });
}

/// Dropping a card below its neighbour reorders the column: same status, index 1.
#[gpui::test]
fn a_card_dragged_within_its_column_asks_for_the_new_place(cx: &mut TestAppContext) {
    fleet_ui_kit::harness::set_recording(true);
    let view = board(3, 0);
    let (card, status) = (view.cards[0].id.clone(), view.board.statuses[0].id.clone());
    let (_state, requests, _harness, visual) = open(view, cx);
    let from = painted("board.column[0].card[0]", visual).center();
    let second = painted("board.column[0].card[1]", visual);
    let to = point(second.center().x, second.bottom() - px(2.0));
    fleet_ui_kit::harness::set_recording(false);

    drag(from, to, visual);

    assert_eq!(
        moves(&requests),
        [RequestBody::MoveCard {
            card_id: card,
            status_id: status,
            index: Some(1),
            cancel_run: false,
        }]
    );
}

/// Repainting the same sample, then wobbling one pixel to either side of a midpoint, must keep
/// the chosen insertion boundary. Without hysteresis those samples alternated the target and
/// rebuilt the old full-height slot on every frame.
#[gpui::test]
fn one_pixel_wobble_at_a_card_midpoint_keeps_the_drag_target(cx: &mut TestAppContext) {
    fleet_ui_kit::harness::set_recording(true);
    let (_state, _requests, harness, visual) = open(board(3, 0), cx);
    let from = painted("board.column[0].card[0]", visual).center();
    let target = painted("board.column[0].card[1]", visual);
    let midpoint = target.center();

    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(
        from + (midpoint - from) * 0.5,
        MouseButton::Left,
        Modifiers::none(),
    );
    visual.simulate_mouse_move(midpoint, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    let chosen = visual.update(|_, cx| {
        harness
            .read(cx)
            .screen
            .drag
            .borrow()
            .target
            .clone()
            .unwrap_or_else(|| panic!("the midpoint did not choose a target"))
    });

    for y in [
        midpoint.y,
        midpoint.y - px(1.0),
        midpoint.y + px(1.0),
        midpoint.y,
    ] {
        visual.simulate_mouse_move(point(midpoint.x, y), MouseButton::Left, Modifiers::none());
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        let current = visual.update(|_, cx| harness.read(cx).screen.drag.borrow().target.clone());
        assert_eq!(current.as_ref(), Some(&chosen));
    }

    visual.simulate_mouse_up(midpoint, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    fleet_ui_kit::harness::set_recording(false);
}

/// The marker is paint-only: even at the end of a scrolled column it must not change any card
/// rectangle or the list's scroll anchor while the drag target is visible.
#[gpui::test]
fn a_drag_marker_keeps_card_bounds_and_bottom_scroll_offset_stable(cx: &mut TestAppContext) {
    fleet_ui_kit::harness::set_recording(true);
    let (_state, _requests, harness, visual) = open(board(40, 0), cx);
    visual.update(|window, cx| {
        let list = harness.read(cx).screen.column_lists[0].clone();
        list.scroll_to_end();
        window.refresh();
    });
    visual.run_until_parked();

    let before = visible_card_bounds(visual);
    let mut cards: Vec<_> = before.iter().collect();
    cards.sort_by_key(|(_, bounds)| bounds.top());
    assert!(
        cards.len() >= 3,
        "a scrolled column should paint several cards"
    );
    let (source_name, source_bounds) = cards[cards.len() - 3];
    let (_, target_bounds) = cards[cards.len() - 1];
    let source_name = source_name.clone();
    let from = source_bounds.center();
    let to = point(target_bounds.center().x, target_bounds.top() + px(1.0));
    let scroll_before =
        visual.update(|_, cx| harness.read(cx).screen.column_lists[0].logical_scroll_top());

    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(
        from + (to - from) * 0.5,
        MouseButton::Left,
        Modifiers::none(),
    );
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    visual.update(|_, cx| {
        assert!(
            harness.read(cx).screen.drag.borrow().target.is_some(),
            "the drag must have a target before geometry is compared"
        );
    });

    let after = visible_card_bounds(visual);
    let unchanged = before.iter().filter(|(name, _)| **name != source_name);
    for (name, before) in unchanged {
        assert_eq!(
            after.get(name),
            Some(before),
            "{name} moved or left the viewport when the marker appeared"
        );
    }
    let scroll_after =
        visual.update(|_, cx| harness.read(cx).screen.column_lists[0].logical_scroll_top());
    assert_eq!(scroll_after.item_ix, scroll_before.item_ix);
    assert_eq!(scroll_after.offset_in_item, scroll_before.offset_in_item);

    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    fleet_ui_kit::harness::set_recording(false);
}

/// A card put back where it was picked up asks for nothing.
#[gpui::test]
fn a_card_dropped_where_it_stands_sends_nothing(cx: &mut TestAppContext) {
    fleet_ui_kit::harness::set_recording(true);
    let (_state, requests, _harness, visual) = open(board(3, 0), cx);
    let tile = painted("board.column[0].card[0]", visual);
    fleet_ui_kit::harness::set_recording(false);

    drag(
        tile.center(),
        point(tile.center().x + px(8.0), tile.center().y + px(4.0)),
        visual,
    );

    assert_eq!(moves(&requests), []);
}

/// On a board whose backend owns the status a drop refuses exactly as `]` does: the read-only
/// toast, and no request.
#[gpui::test]
fn a_drop_on_a_read_only_status_toasts_like_the_key(cx: &mut TestAppContext) {
    fleet_ui_kit::harness::set_recording(true);
    let mut view = board(2, 1);
    view.board.backend.kind = "jira".to_owned();
    view.board.sync.readonly_fields = vec!["status_id".to_owned()];
    let (state, requests, _harness, visual) = open(view, cx);
    let from = painted("board.column[0].card[0]", visual).center();
    let to = painted("board.column[2].card[0]", visual).center();
    fleet_ui_kit::harness::set_recording(false);

    drag(from, to, visual);

    assert_eq!(moves(&requests), []);
    let expected = visual.update(|_, cx| {
        readonly_message(
            state.read(cx),
            &crate::dialogs::card_picker::PickerKind::Status,
        )
    });
    let toasts: Vec<String> = visual.update(|_, cx| {
        state
            .read(cx)
            .toasts
            .iter()
            .map(|toast| toast.toast.text.to_string())
            .collect()
    });
    assert!(
        expected.is_some_and(|message| toasts.contains(&message)),
        "the drop says what `]` says: {toasts:?}"
    );
}

/// The drawn slot maps to the daemon's index over the whole column, hidden cards included.
#[test]
fn a_filtered_column_drops_before_the_card_drawn_below_the_slot() {
    let mut view = board(0, 4);
    view.cards[1].title = "Hidden".to_owned();
    view.cards[3].title = "Hidden too".to_owned();
    let dragged = view.cards[2].id.clone();
    // Drawn under the filter `Card`: cards 0 and 2. Slot 0 is above card 0.
    assert_eq!(
        drop_destination(&view, "Card", &dragged, 2, 0),
        Some(Destination::At {
            column: 2,
            index: 0
        })
    );
    // Slot 1 or 2 is where card 2 is drawn already.
    assert_eq!(drop_destination(&view, "Card", &dragged, 2, 1), None);
    assert_eq!(drop_destination(&view, "Card", &dragged, 2, 2), None);
    // Another card dropped under card 2 lands after it, before the hidden card 3.
    let other = view.cards[0].id.clone();
    assert_eq!(
        drop_destination(&view, "Card", &other, 2, 2),
        Some(Destination::At {
            column: 2,
            index: 2
        })
    );
}

/// The answer to a placed move carries the moved card alone; its neighbours are renumbered the
/// way the daemon renumbered them, so the column draws the new order before the reload lands.
#[test]
fn a_placed_move_reorders_the_neighbours_it_does_not_carry() {
    let mut state = AppState::new("/tmp/fleet-board-placed", Instant::now());
    let view = board(3, 0);
    let status = view.board.statuses[0].id.clone();
    let mut moved = view.cards[0].clone();
    state.board.view = Some(view);
    // What the daemon answers for card 0 put at index 1: its new position, nothing else.
    moved.position = 10;
    moved.updated_at = "2026-09-06T12:01:00Z".into();
    state.apply_placed_card(moved, 1);

    let view = state.board().unwrap_or_else(|| panic!("no board"));
    let order: Vec<String> = crate::views::board_screen::visible_cards(view, &status, "")
        .iter()
        .map(|card| card.title.clone())
        .collect();
    assert_eq!(order, ["Card 1", "Card 0", "Card 2"]);
}
